//! Daemon state: the single runtime owner of the fans and the profile
//! (SPEC 8).
//!
//! Plain synchronous logic on top of `alienfan-core`. `service.rs` exposes
//! it on D-Bus and `main.rs` drives the clock, so tests can run it directly
//! on a fake sysfs tree.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use alienfan_core::controller::{Controller, Mode};
use alienfan_core::curve::{DEFAULT_HYSTERESIS_C, DEFAULT_RAMP_DOWN_PER_S, DEFAULT_RAMP_UP_PER_S};
use alienfan_core::plan::effective_profile;
use alienfan_core::sysfs::{is_writable, read_temp};
use alienfan_core::{
    Boost, ConfigFile, Control, ControlKind, Curve, CurvePoint, FanId, FanPair, Hardware, Health,
    OverrideUntil, PowerSource, Preset, PresetConfig, Profile, SysfsRoot,
};
use alienfan_proto::{CurveOptions, Error, FanTelemetry, Points, PresetDict, Temps};
use tracing::{debug, error, info, warn};
use zvariant::Value;

type CoreError = alienfan_core::Error;
pub type Result<T = ()> = std::result::Result<T, Error>;

/// Maps a core error to the D-Bus error of SPEC 9.
#[allow(clippy::needless_pass_by_value)] // shaped for `map_err`
fn dbus_error(e: CoreError) -> Error {
    let message = e.to_string();
    match e {
        CoreError::Invalid(_) | CoreError::Config(_) => Error::InvalidArgument(message),
        CoreError::CurveInUse { .. } => Error::CurveInUse(message),
        CoreError::NoPermission { .. } => Error::PermissionDenied(message),
        CoreError::NoDriver(_) | CoreError::Io { .. } => Error::HardwareUnavailable(message),
    }
}

fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidArgument(message.into())
}

/// What the D-Bus properties show.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Props {
    pub health: String,
    pub health_message: String,
    pub power_source: String,
    pub profile: String,
    pub available_profiles: Vec<String>,
    pub control: String,
    pub active_curve: String,
    pub override_active: bool,
    pub boost_requires_custom: bool,
    pub override_until: String,
    pub emergency_temp_c: f64,
}

pub struct State {
    root: SysfsRoot,
    config_path: PathBuf,
    file: ConfigFile,
    /// Set while the file on disk is invalid and an older config is in use.
    config_error: Option<String>,
    config_stamp: Option<(SystemTime, u64)>,
    hw: Option<Hardware>,
    power: PowerSource,
    /// Manual change on top of the default (SPEC 8.2).
    override_preset: Option<Preset>,
    controller: Controller,
    /// Name of the curve the controller runs, to keep its ramps on reapply.
    running_curve: Option<String>,
    sensors: FanPair<Option<PathBuf>>,
    no_permission: bool,
    last_profile: Option<Profile>,
    control_health: Health,
    telemetry: (Vec<FanTelemetry>, Temps),
    start: Instant,
}

impl State {
    /// SPEC 8.1: load the config, find the hardware, read the power source
    /// and apply its default.
    pub fn new(root: SysfsRoot, config_path: PathBuf, now: Instant) -> Self {
        let (file, config_error) = match ConfigFile::load(&config_path) {
            Ok(file) => (file, None),
            Err(e) => {
                error!("config inválida, usando os padrões embutidos: {e}");
                (ConfigFile::default(), Some(e.to_string()))
            }
        };
        let hw = Hardware::discover(&root).map_err(|e| warn!("{e}")).ok();
        let power = root
            .power_source()
            .ok()
            .flatten()
            .unwrap_or(PowerSource::Ac);
        let current = hw.as_ref().map(read_boost).unwrap_or_default();
        let controller = Controller::new(
            Mode::Firmware,
            file.config().hardware.emergency_temp_c,
            current,
        );
        let mut state = Self {
            config_stamp: stamp(&config_path),
            root,
            config_path,
            file,
            config_error,
            hw,
            power,
            override_preset: None,
            controller,
            running_curve: None,
            sensors: FanPair::default(),
            no_permission: false,
            last_profile: None,
            control_health: Health::Ok,
            telemetry: (Vec::new(), Temps::new()),
            start: now,
        };
        info!(power = %power, "alienfand iniciado");
        state.apply_or_log(now);
        state
    }

    pub fn tick_period(&self) -> Duration {
        Duration::from_millis(u64::from(self.file.config().daemon.tick_ms))
    }

    pub fn telemetry(&self) -> (Vec<FanTelemetry>, Temps) {
        self.telemetry.clone()
    }

    fn effective(&self) -> Preset {
        self.override_preset
            .clone()
            .unwrap_or_else(|| self.file.config().defaults.get(self.power).preset())
    }

    fn hardware(&self) -> Result<&Hardware> {
        self.hw
            .as_ref()
            .ok_or_else(|| Error::HardwareUnavailable("driver alienware_wmi não encontrado".into()))
    }

    /// One period of the loop: config, power source, driver, then control.
    pub fn tick(&mut self, now: Instant) {
        if stamp(&self.config_path) != self.config_stamp {
            let _ = self.reload(now);
        }
        if self.update_power() {
            self.apply_or_log(now);
            return;
        }
        if self.hw.as_ref().is_some_and(|hw| !hw.is_present()) {
            warn!("o driver sumiu (módulo recarregado?); procurando de novo");
            self.hw = None;
        }
        if self.hw.is_none()
            && let Ok(hw) = Hardware::discover(&self.root)
        {
            info!("driver encontrado");
            self.hw = Some(hw);
            self.controller.invalidate();
            self.apply_or_log(now);
            return;
        }
        // A profile change from outside (a key, sudo) resets the boost.
        let profile = self.hw.as_ref().and_then(|hw| hw.profile().ok());
        if profile.is_some() && profile != self.last_profile {
            if let Some(p) = profile {
                info!(profile = %p, "perfil trocado fora do alienfan; reescrevendo o boost");
            }
            self.last_profile = profile;
            self.controller.invalidate();
        }
        // Write failures are logged and reflected in Health.
        let _ = self.control_tick(now);
    }

    /// Makes the hardware follow the effective preset: profile first, then
    /// the boost through the controller (SPEC 6.4).
    fn apply(&mut self, now: Instant) -> Result {
        let preset = self.effective();
        match &preset.control {
            Control::Firmware => self.controller.set_mode(Mode::Firmware),
            Control::Fixed(boost) => self.controller.set_mode(Mode::Fixed(*boost)),
            Control::Curve(name) => {
                let curve = self.file.config().curve(name).map_err(dbus_error)?.clone();
                if self.running_curve.as_deref() == Some(name.as_str()) {
                    self.controller.update_curve(curve);
                } else {
                    self.controller.set_curve(curve);
                }
            }
        }
        self.running_curve = match &preset.control {
            Control::Curve(name) => Some(name.clone()),
            _ => None,
        };

        let profile = effective_profile(&preset, self.file.config().hardware.boost_requires_custom);
        let hw = self.hardware()?.clone();
        if hw.profile().map_err(dbus_error)? != profile {
            if let Err(e) = hw.set_profile(profile) {
                return Err(self.write_failed(e));
            }
            info!(profile = %profile, "perfil aplicado");
            self.controller.invalidate();
        }
        self.last_profile = Some(profile);
        self.control_tick(now)
    }

    /// For internal events: log failures. An override that no longer applies
    /// (its curve was removed by hand) gives way to the default.
    fn apply_or_log(&mut self, now: Instant) {
        match self.apply(now) {
            Ok(()) => {}
            Err(Error::InvalidArgument(e)) if self.override_preset.is_some() => {
                warn!("override descartado: {e}");
                self.override_preset = None;
                self.apply_or_log(now);
            }
            Err(e) => warn!("não foi possível aplicar o preset: {e}"),
        }
    }

    fn write_failed(&mut self, e: CoreError) -> Error {
        match &e {
            CoreError::NoPermission { .. } => {
                if !self.no_permission {
                    error!("{e}; rode `alienfan doctor`");
                }
                self.no_permission = true;
            }
            CoreError::NoDriver(_) => {
                warn!("{e}");
                self.hw = None;
            }
            _ => warn!("{e}"),
        }
        self.controller.invalidate();
        dbus_error(e)
    }

    /// Reads the sensors, runs the controller, writes its boosts and
    /// refreshes the telemetry. Fails if the boost could not be written.
    fn control_tick(&mut self, now: Instant) -> Result {
        let temps = FanPair::new(self.read_sensor(FanId::Cpu), self.read_sensor(FanId::Gpu));
        let out = self
            .controller
            .tick(now.saturating_duration_since(self.start), temps);
        if out.health != self.control_health {
            match out.health {
                Health::Emergency => warn!(?temps, "emergência: boost 255 nas duas ventoinhas"),
                Health::Degraded => warn!("sensor sem leitura: o firmware assumiu a ventoinha"),
                _ => info!("controle normal"),
            }
            self.control_health = out.health;
        }

        let mut result = Ok(());
        if let Some(hw) = self.hw.clone() {
            if self.no_permission && writable(&hw) {
                info!("permissão de escrita recuperada");
                self.no_permission = false;
                // The writes of this tick were planned as if they succeed.
                self.controller.invalidate();
            } else if self.no_permission {
                result = Err(Error::PermissionDenied(
                    "sem permissão de escrita no boost; rode `alienfan doctor`".into(),
                ));
            } else {
                for (fan, boost) in out.writes {
                    debug!(fan = %fan, boost = boost.0, "boost");
                    if let Err(e) = hw.set_boost(fan, boost, true) {
                        result = Err(self.write_failed(e));
                        break;
                    }
                }
            }
        }
        self.telemetry = self.read_telemetry(out.target, temps);
        result
    }

    fn read_sensor(&mut self, fan: FanId) -> Option<f64> {
        if self.sensors[fan].is_none() {
            self.sensors[fan] = self
                .root
                .resolve_sensor(&self.file.config().sensors[fan])
                .ok();
        }
        let path = self.sensors[fan].clone()?;
        let temp = read_temp(&path).ok();
        if temp.is_none() {
            self.sensors[fan] = None;
        }
        temp
    }

    fn read_telemetry(
        &self,
        target: FanPair<Boost>,
        temps: FanPair<Option<f64>>,
    ) -> (Vec<FanTelemetry>, Temps) {
        let sensors = &self.file.config().sensors;
        let fans = self
            .hw
            .iter()
            .flat_map(|hw| FanId::ALL.iter().map(move |&fan| (hw, fan)))
            .filter_map(|(hw, fan)| {
                let r = hw.fan(fan).ok()?;
                Some(FanTelemetry {
                    id: fan.as_str().into(),
                    label: r.label,
                    rpm: r.rpm,
                    rpm_max: r.rpm_max,
                    boost: r.boost.0,
                    target_boost: target[fan].0,
                    temp_c: temps[fan],
                    sensor: sensors[fan].to_string(),
                })
            })
            .collect();
        (fans, self.root.read_all_temps().into_iter().collect())
    }

    /// Returns whether the power source changed. Ends the override when
    /// `override_until = "power-change"`.
    fn update_power(&mut self) -> bool {
        let Ok(Some(power)) = self.root.power_source() else {
            return false;
        };
        if power == self.power {
            return false;
        }
        info!(from = %self.power, to = %power, "fonte de energia mudou");
        self.power = power;
        if self.override_preset.is_some()
            && self.file.config().daemon.override_until == OverrideUntil::PowerChange
        {
            info!("override encerrado pela troca de energia");
            self.override_preset = None;
        }
        true
    }

    /// `UPower` said the power source changed: act now instead of next tick.
    pub fn check_power(&mut self, now: Instant) {
        if self.update_power() {
            self.apply_or_log(now);
        }
    }

    /// After a suspend: find the hardware again and reapply everything.
    pub fn resume(&mut self, now: Instant) {
        info!("voltou da suspensão: reaplicando");
        self.hw = Hardware::discover(&self.root)
            .map_err(|e| warn!("{e}"))
            .ok();
        self.sensors = FanPair::default();
        self.last_profile = None;
        self.controller.invalidate();
        self.update_power();
        self.apply_or_log(now);
    }

    /// SIGTERM/SIGINT: hand the fans back to the firmware (SPEC 8.1).
    pub fn shutdown(&mut self) {
        let manual =
            self.effective().control != Control::Firmware || self.controller.in_emergency();
        match &self.hw {
            Some(hw) if manual => {
                for fan in FanId::ALL.iter().copied() {
                    let _ = hw.set_boost(fan, Boost::MIN, true);
                }
                info!("encerrado: boost 0 nas duas ventoinhas, o firmware reassume");
            }
            _ => info!("encerrado"),
        }
    }

    pub fn reload(&mut self, now: Instant) -> Result {
        self.config_stamp = stamp(&self.config_path);
        match ConfigFile::load(&self.config_path) {
            Ok(file) => {
                info!("config recarregada");
                self.file = file;
                self.config_error = None;
            }
            Err(e) => {
                error!("config inválida, mantendo a anterior: {e}");
                self.config_error = Some(e.to_string());
                return Err(invalid(e.to_string()));
            }
        }
        self.controller
            .set_emergency_temp(self.file.config().hardware.emergency_temp_c);
        self.sensors = FanPair::default();
        self.apply_or_log(now);
        Ok(())
    }

    /// Edits a copy of the config, saves it, then keeps it.
    fn edit_config(
        &mut self,
        edit: impl FnOnce(&mut ConfigFile) -> std::result::Result<(), CoreError>,
    ) -> Result {
        let mut file = self.file.clone();
        edit(&mut file).map_err(dbus_error)?;
        file.save(&self.config_path).map_err(|e| {
            error!("{e}");
            Error::ConfigWriteFailed(format!(
                "não foi possível gravar {}: {e}",
                self.config_path.display()
            ))
        })?;
        self.file = file;
        self.config_error = None;
        self.config_stamp = stamp(&self.config_path);
        Ok(())
    }

    fn set_override(&mut self, preset: Preset, now: Instant) -> Result {
        info!(?preset, "override");
        self.override_preset = Some(preset);
        self.apply(now)
    }

    pub fn set_profile(&mut self, name: &str, now: Instant) -> Result {
        let profile: Profile = name.parse().map_err(dbus_error)?;
        if !self
            .hardware()?
            .choices()
            .map_err(dbus_error)?
            .contains(&profile)
        {
            return Err(invalid(format!("perfil \"{profile}\" não está disponível")));
        }
        let mut preset = self.effective();
        preset.profile = profile;
        self.set_override(preset, now)
    }

    /// The boost the fans have now, as a starting point for manual control.
    fn manual_boost(&self, preset: &Preset) -> FanPair<Boost> {
        match preset.control {
            Control::Fixed(boost) => boost,
            _ => self.controller.output(),
        }
    }

    pub fn set_fixed_boost(&mut self, fan: &str, boost: u8, now: Instant) -> Result {
        let fans = parse_fans(fan)?;
        let preset = self.effective();
        let mut pair = self.manual_boost(&preset);
        for fan in fans {
            pair[fan] = Boost(boost);
        }
        self.set_override(
            Preset {
                profile: preset.profile,
                control: Control::Fixed(pair),
            },
            now,
        )
    }

    pub fn set_control(&mut self, control: &str, curve: &str, now: Instant) -> Result {
        let kind: ControlKind = control.parse().map_err(dbus_error)?;
        let preset = self.effective();
        let control = match kind {
            ControlKind::Firmware => Control::Firmware,
            ControlKind::Fixed => Control::Fixed(self.manual_boost(&preset)),
            ControlKind::Curve => {
                let name = if curve.is_empty() {
                    self.file.config().defaults.get(self.power).curve.clone()
                } else {
                    curve.to_owned()
                };
                self.file.config().curve(&name).map_err(dbus_error)?;
                Control::Curve(name)
            }
        };
        self.set_override(
            Preset {
                profile: preset.profile,
                control,
            },
            now,
        )
    }

    pub fn restore_default(&mut self, now: Instant) -> Result {
        if self.override_preset.take().is_some() {
            info!("override removido: voltando ao padrão");
        }
        self.apply(now)
    }

    /// Stores the effective preset as default. Saving the default of the
    /// current source also ends the override, since they now match.
    pub fn save_as_default(&mut self, target: &str, now: Instant) -> Result {
        let targets = parse_targets(target)?;
        let preset = self.effective();
        self.edit_config(|file| {
            for &source in &targets {
                let mut stored = file.config().defaults.get(source).clone();
                stored.assign(&preset);
                file.set_default(source, &stored)?;
            }
            Ok(())
        })?;
        info!(target, "padrão salvo");
        if targets.contains(&self.power) {
            self.override_preset = None;
        }
        self.apply(now)
    }

    /// Edits a default. Without an override, the hardware follows the
    /// default of the current source, so that one is applied at once.
    pub fn set_default(&mut self, target: &str, dict: &PresetDict, now: Instant) -> Result {
        let targets = parse_targets(target)?;
        let choices = self
            .hw
            .as_ref()
            .and_then(|hw| hw.choices().ok())
            .unwrap_or_else(|| Profile::ALL.to_vec());
        self.edit_config(|file| {
            for &source in &targets {
                let mut stored = file.config().defaults.get(source).clone();
                merge_preset(&mut stored, dict, &choices)?;
                file.set_default(source, &stored)?;
            }
            Ok(())
        })?;
        if self.override_preset.is_none() && targets.contains(&self.power) {
            self.apply(now)?;
        }
        Ok(())
    }

    pub fn get_defaults(&self) -> (PresetDict, PresetDict) {
        let defaults = &self.file.config().defaults;
        (preset_dict(&defaults.ac), preset_dict(&defaults.battery))
    }

    pub fn list_curves(&self) -> Vec<String> {
        self.file.config().curves.keys().cloned().collect()
    }

    pub fn get_curve(&self, name: &str, fan: &str) -> Result<Points> {
        let fan: FanId = fan.parse().map_err(dbus_error)?;
        let curve = self.file.config().curve(name).map_err(dbus_error)?;
        Ok(curve
            .points(fan)
            .iter()
            .map(|p| (p.temp_c, p.boost.0))
            .collect())
    }

    pub fn get_curve_options(&self, name: &str) -> Result<CurveOptions> {
        let curve = self.file.config().curve(name).map_err(dbus_error)?;
        Ok(CurveOptions {
            hysteresis_c: Some(curve.hysteresis_c),
            ramp_up_per_s: Some(curve.ramp_up_per_s),
            ramp_down_per_s: Some(curve.ramp_down_per_s),
        })
    }

    /// Creates or replaces a curve; missing options keep their value.
    pub fn save_curve(
        &mut self,
        name: &str,
        cpu: &[(f64, u8)],
        gpu: &[(f64, u8)],
        options: &CurveOptions,
    ) -> Result {
        let existing = self.file.config().curves.get(name);
        let curve = Curve::new(
            FanPair::new(points(cpu), points(gpu)),
            options
                .hysteresis_c
                .or(existing.map(|c| c.hysteresis_c))
                .unwrap_or(DEFAULT_HYSTERESIS_C),
            options
                .ramp_up_per_s
                .or(existing.map(|c| c.ramp_up_per_s))
                .unwrap_or(DEFAULT_RAMP_UP_PER_S),
            options
                .ramp_down_per_s
                .or(existing.map(|c| c.ramp_down_per_s))
                .unwrap_or(DEFAULT_RAMP_DOWN_PER_S),
        )
        .map_err(dbus_error)?;
        self.edit_config(|file| file.set_curve(name, &curve))?;
        info!(name, "curva salva");
        if self.running_curve.as_deref() == Some(name) {
            self.controller.update_curve(curve);
        }
        Ok(())
    }

    pub fn delete_curve(&mut self, name: &str) -> Result {
        if matches!(&self.override_preset, Some(Preset { control: Control::Curve(n), .. }) if n == name)
        {
            return Err(Error::CurveInUse(format!(
                "a curva \"{name}\" está em uso no override atual"
            )));
        }
        self.edit_config(|file| file.remove_curve(name))?;
        info!(name, "curva apagada");
        Ok(())
    }

    pub fn set_daemon_option(&mut self, name: &str, value: &Value<'_>) -> Result {
        match (name, value) {
            ("override_until", Value::Str(s)) => {
                let until: OverrideUntil = s.as_str().parse().map_err(dbus_error)?;
                self.edit_config(|file| file.set_override_until(until))?;
                info!(override_until = %until, "opção alterada");
                Ok(())
            }
            ("override_until", _) => Err(invalid("override_until precisa de uma string")),
            _ => Err(invalid(format!("opção desconhecida: \"{name}\""))),
        }
    }

    fn health(&self) -> (Health, String) {
        if self.hw.is_none() {
            return (
                Health::NoDriver,
                "Driver alienware_wmi não encontrado. Rode `alienfan doctor`.".into(),
            );
        }
        if self.control_health == Health::Emergency {
            let limit = self.file.config().hardware.emergency_temp_c;
            return (
                Health::Emergency,
                format!("Temperatura acima de {limit} °C: ventoinhas no máximo."),
            );
        }
        if self.no_permission {
            return (
                Health::NoPermission,
                "Sem permissão de escrita no perfil e no boost. Rode `alienfan doctor`.".into(),
            );
        }
        if let Some(e) = &self.config_error {
            return (
                Health::Degraded,
                format!("Config inválida ({e}). Usando a última válida."),
            );
        }
        if self.control_health == Health::Degraded {
            return (
                Health::Degraded,
                "Sensor de temperatura sem leitura: o firmware assumiu a ventoinha.".into(),
            );
        }
        (Health::Ok, String::new())
    }

    pub fn props(&self) -> Props {
        let (health, health_message) = self.health();
        let preset = self.effective();
        let config = self.file.config();
        Props {
            health: health.as_str().into(),
            health_message,
            power_source: self.power.as_str().into(),
            profile: self
                .hw
                .as_ref()
                .and_then(|hw| hw.profile().ok())
                .map(|p| p.as_str().to_owned())
                .unwrap_or_default(),
            available_profiles: self
                .hw
                .as_ref()
                .and_then(|hw| hw.choices().ok())
                .unwrap_or_default()
                .iter()
                .map(|p| p.as_str().to_owned())
                .collect(),
            control: preset.control.kind().as_str().into(),
            active_curve: match &preset.control {
                Control::Curve(name) => name.clone(),
                _ => String::new(),
            },
            override_active: self.override_preset.is_some(),
            boost_requires_custom: config.hardware.boost_requires_custom,
            override_until: config.daemon.override_until.as_str().into(),
            emergency_temp_c: config.hardware.emergency_temp_c,
        }
    }
}

fn read_boost(hw: &Hardware) -> FanPair<Boost> {
    FanPair::new(
        hw.boost(FanId::Cpu).unwrap_or_default(),
        hw.boost(FanId::Gpu).unwrap_or_default(),
    )
}

fn writable(hw: &Hardware) -> bool {
    is_writable(&hw.profile_path()) && FanId::ALL.iter().all(|&f| is_writable(&hw.boost_path(f)))
}

/// Modification time and size: enough to notice edits and atomic renames.
fn stamp(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

fn points(raw: &[(f64, u8)]) -> Vec<CurvePoint> {
    raw.iter().map(|&(t, b)| CurvePoint::new(t, b)).collect()
}

fn parse_fans(fan: &str) -> Result<Vec<FanId>> {
    if fan == "all" {
        Ok(FanId::ALL.to_vec())
    } else {
        Ok(vec![fan.parse().map_err(dbus_error)?])
    }
}

fn parse_targets(target: &str) -> Result<Vec<PowerSource>> {
    match target {
        "both" => Ok(PowerSource::ALL.to_vec()),
        other => Ok(vec![other.parse().map_err(|_| {
            invalid(format!(
                "destino inválido: \"{other}\" (use ac, battery ou both)"
            ))
        })?]),
    }
}

fn preset_dict(p: &PresetConfig) -> PresetDict {
    PresetDict {
        profile: Some(p.profile.as_str().into()),
        control: Some(p.control.as_str().into()),
        fixed_cpu: Some(p.fixed.cpu.0),
        fixed_gpu: Some(p.fixed.gpu.0),
        curve: Some(p.curve.clone()),
    }
}

fn merge_preset(
    stored: &mut PresetConfig,
    dict: &PresetDict,
    choices: &[Profile],
) -> std::result::Result<(), CoreError> {
    if let Some(profile) = &dict.profile {
        let profile: Profile = profile.parse()?;
        if !choices.contains(&profile) {
            return Err(CoreError::Invalid(format!(
                "perfil \"{profile}\" não está disponível"
            )));
        }
        stored.profile = profile;
    }
    if let Some(control) = &dict.control {
        stored.control = control.parse()?;
    }
    if let Some(cpu) = dict.fixed_cpu {
        stored.fixed.cpu = Boost(cpu);
    }
    if let Some(gpu) = dict.fixed_gpu {
        stored.fixed.gpu = Boost(gpu);
    }
    if let Some(curve) = &dict.curve {
        stored.curve.clone_from(curve);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
