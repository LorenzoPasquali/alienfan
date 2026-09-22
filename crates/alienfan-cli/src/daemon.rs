//! Commands sent to `alienfand` when it is on the session bus (SPEC 10).

use std::future::Future;

use alienfan_core::{
    Boost, ControlKind, Curve, CurvePoint, FanId, FanPair, Health, PowerSource, PresetConfig,
    Profile,
};
use alienfan_proto::{
    AlienFan1Proxy, BUS_NAME, CurveOptions, PresetDict, error_message, error_name,
};
use tokio::runtime::Runtime;
use zbus::proxy::CacheProperties;

use crate::curves::{self, SetArgs};
use crate::direct::{boost_text, preset_text};
use crate::status::{FanStatus, Status};
use crate::{CliResult, Code, Failure, FanArg};

pub struct Daemon {
    rt: Runtime,
    proxy: AlienFan1Proxy<'static>,
}

impl Daemon {
    /// Connects only if the daemon already runs: a CLI call must not start
    /// it through D-Bus activation.
    pub fn connect() -> Option<Self> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?;
        let proxy = rt.block_on(async {
            let conn = zbus::Connection::session().await.ok()?;
            let dbus = zbus::fdo::DBusProxy::new(&conn).await.ok()?;
            let running = dbus.name_has_owner(BUS_NAME.try_into().ok()?).await.ok()?;
            if !running {
                return None;
            }
            AlienFan1Proxy::builder(&conn)
                .cache_properties(CacheProperties::No)
                .build()
                .await
                .ok()
        })?;
        Some(Self { rt, proxy })
    }

    fn call<T>(&self, f: impl Future<Output = zbus::Result<T>>) -> CliResult<T> {
        self.rt.block_on(f).map_err(|e| failure(&e))
    }

    pub fn status(&self) -> CliResult<Status> {
        let p = &self.proxy;
        let (fans, temps) = self.call(p.get_telemetry())?;
        let mut temps: Vec<(String, f64)> = temps.into_iter().collect();
        temps.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(Status {
            daemon: true,
            health: parse_or(&self.call(p.health())?, Health::Degraded),
            health_message: self.call(p.health_message())?,
            power_source: parse_or(&self.call(p.power_source())?, PowerSource::Ac),
            profile: self.call(p.profile())?.parse().ok(),
            available_profiles: self.profiles()?,
            control: parse_or(&self.call(p.control())?, ControlKind::Firmware),
            active_curve: self.call(p.active_curve())?,
            override_active: self.call(p.override_active())?,
            boost_requires_custom: self.call(p.boost_requires_custom())?,
            fans: fans
                .into_iter()
                .filter_map(|f| {
                    Some(FanStatus {
                        id: f.id.parse().ok()?,
                        label: f.label,
                        rpm: f.rpm,
                        rpm_max: f.rpm_max,
                        boost: f.boost,
                        boost_pct: Boost(f.boost).pct(),
                        target_boost: f.target_boost,
                        temp_c: f.temp_c,
                        sensor: f.sensor,
                    })
                })
                .collect(),
            temps,
        })
    }

    fn profiles(&self) -> CliResult<Vec<Profile>> {
        let names = self.call(self.proxy.available_profiles())?;
        Ok(names.iter().filter_map(|n| n.parse().ok()).collect())
    }

    pub fn profile_list(&self) -> CliResult {
        let current = self.call(self.proxy.profile())?.parse::<Profile>().ok();
        for p in self.profiles()? {
            let mark = if Some(p) == current { '*' } else { ' ' };
            println!("{mark} {:<22} {}", p.as_str(), p.label());
        }
        Ok(())
    }

    pub fn profile_set(&self, profile: Profile) -> CliResult {
        self.call(self.proxy.set_profile(profile.as_str()))?;
        println!("Perfil: {} ({profile})", profile.label());
        Ok(())
    }

    pub fn boost(&self, fan: FanArg, value: Boost) -> CliResult {
        let fan = match fan {
            FanArg::Cpu => "cpu",
            FanArg::Gpu => "gpu",
            FanArg::All => "all",
        };
        self.call(self.proxy.set_fixed_boost(fan, value.0))?;
        let status = self.status()?;
        let boost = FanPair::new(
            fan_boost(&status, FanId::Cpu),
            fan_boost(&status, FanId::Gpu),
        );
        println!("Boost: {}", boost_text(boost));
        Ok(())
    }

    pub fn control(&self, kind: ControlKind, curve: Option<&str>) -> CliResult {
        self.call(self.proxy.set_control(kind.as_str(), curve.unwrap_or("")))?;
        match kind {
            ControlKind::Curve => {
                let name = self.call(self.proxy.active_curve())?;
                println!("Controle: Curva {name}");
            }
            other => println!("Controle: {}", other.label()),
        }
        Ok(())
    }

    fn curve(&self, name: &str) -> CliResult<Curve> {
        let points = |fan: &'static str| -> CliResult<Vec<CurvePoint>> {
            let raw = self.call(self.proxy.get_curve(name, fan))?;
            Ok(raw
                .into_iter()
                .map(|(t, b)| CurvePoint::new(t, b))
                .collect())
        };
        let o = self.call(self.proxy.get_curve_options(name))?;
        let curve = Curve::new(
            FanPair::new(points("cpu")?, points("gpu")?),
            o.hysteresis_c.unwrap_or_default(),
            o.ramp_up_per_s.unwrap_or_default(),
            o.ramp_down_per_s.unwrap_or_default(),
        )?;
        Ok(curve)
    }

    fn defaults(&self) -> CliResult<(PresetConfig, PresetConfig)> {
        let (ac, battery) = self.call(self.proxy.get_defaults())?;
        Ok((preset_config(&ac)?, preset_config(&battery)?))
    }

    pub fn curve_list(&self) -> CliResult {
        let (ac, battery) = self.defaults()?;
        let entries: Vec<_> = self
            .call(self.proxy.list_curves())?
            .into_iter()
            .map(|name| {
                let users = [(PowerSource::Ac, &ac), (PowerSource::Battery, &battery)]
                    .into_iter()
                    .filter(|(_, p)| p.control == ControlKind::Curve && p.curve == name)
                    .map(|(s, _)| s)
                    .collect();
                (name, users)
            })
            .collect();
        curves::print_list(&entries);
        Ok(())
    }

    pub fn curve_show(&self, name: &str) -> CliResult {
        curves::print_curve(name, &self.curve(name)?);
        Ok(())
    }

    pub fn curve_set(&self, args: &SetArgs) -> CliResult {
        let exists = self.call(self.proxy.list_curves())?.contains(&args.name);
        let to_raw = |pts: &[CurvePoint]| -> Vec<(f64, u8)> {
            pts.iter().map(|p| (p.temp_c, p.boost.0)).collect()
        };
        let cpu = to_raw(&args.cpu.0);
        let gpu = match (&args.gpu, exists) {
            (Some(points), _) => to_raw(&points.0),
            (None, true) => self.call(self.proxy.get_curve(&args.name, "gpu"))?,
            (None, false) => cpu.clone(),
        };
        let options = CurveOptions {
            hysteresis_c: args.hysteresis,
            ramp_up_per_s: args.ramp_up,
            ramp_down_per_s: args.ramp_down,
        };
        self.call(
            self.proxy
                .save_curve_with_options(&args.name, &cpu, &gpu, &options),
        )?;
        let verb = if exists { "atualizada" } else { "criada" };
        println!("Curva \"{}\" {verb}.", args.name);
        Ok(())
    }

    pub fn default_show(&self) -> CliResult {
        let (ac, battery) = self.defaults()?;
        for (source, preset) in [(PowerSource::Ac, &ac), (PowerSource::Battery, &battery)] {
            println!(
                "{:<12} {}",
                format!("{}:", source.label()),
                preset_text(preset)
            );
        }
        let until = self.call(self.proxy.override_until())?;
        println!("Override     termina em: {until}");
        Ok(())
    }

    pub fn default_save(&self, ac: bool, battery: bool) -> CliResult {
        let target = match (ac, battery) {
            (true, true) => "both".to_owned(),
            (true, false) => "ac".to_owned(),
            (false, true) => "battery".to_owned(),
            (false, false) => self.call(self.proxy.power_source())?,
        };
        self.call(self.proxy.save_as_default(&target))?;
        let (ac, battery) = self.defaults()?;
        for (source, preset) in [(PowerSource::Ac, &ac), (PowerSource::Battery, &battery)] {
            if target == "both" || target == source.as_str() {
                println!("Padrão salvo ({}): {}", source.label(), preset_text(preset));
            }
        }
        Ok(())
    }

    pub fn default_reset(&self) -> CliResult {
        self.call(self.proxy.restore_default())?;
        let profile = self.call(self.proxy.profile())?;
        println!("Padrão restaurado: perfil {profile}");
        Ok(())
    }
}

fn fan_boost(status: &Status, fan: FanId) -> Boost {
    status
        .fans
        .iter()
        .find(|f| f.id == fan)
        .map_or(Boost::MIN, |f| Boost(f.boost))
}

fn parse_or<T: std::str::FromStr>(s: &str, fallback: T) -> T {
    s.parse().unwrap_or(fallback)
}

fn preset_config(d: &PresetDict) -> CliResult<PresetConfig> {
    let bad = || Failure::new(Code::Daemon, "resposta inesperada do daemon em GetDefaults");
    Ok(PresetConfig {
        profile: d.profile.as_deref().ok_or_else(bad)?.parse()?,
        control: d.control.as_deref().ok_or_else(bad)?.parse()?,
        fixed: FanPair::new(
            Boost(d.fixed_cpu.unwrap_or_default()),
            Boost(d.fixed_gpu.unwrap_or_default()),
        ),
        curve: d.curve.clone().unwrap_or_default(),
    })
}

/// Maps a D-Bus error to the exit codes of SPEC 10.
fn failure(e: &zbus::Error) -> Failure {
    let code = match error_name(e) {
        Some("InvalidArgument" | "CurveInUse") => Code::InvalidArgument,
        Some("PermissionDenied") => Code::NoPermission,
        Some("HardwareUnavailable") => Code::NoHardware,
        Some("ConfigWriteFailed") => Code::Generic,
        _ => Code::Daemon,
    };
    Failure::new(code, error_message(e))
}
