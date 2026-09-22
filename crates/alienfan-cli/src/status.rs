//! `alienfan status`.

use std::fmt::Write as _;
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use alienfan_core::config::Config;
use alienfan_core::sysfs::{is_writable, read_temp};
use alienfan_core::{ControlKind, FanId, Hardware, Health, PowerSource, Profile};
use serde::{Serialize, Serializer};

use crate::{CliResult, Code, Ctx, Failure};

/// Everything `status` shows. Field names match the D-Bus properties.
#[derive(Debug, Serialize)]
pub struct Status {
    pub daemon: bool,
    pub health: Health,
    pub health_message: String,
    pub power_source: PowerSource,
    pub profile: Option<Profile>,
    pub available_profiles: Vec<Profile>,
    pub control: ControlKind,
    pub active_curve: String,
    pub override_active: bool,
    pub boost_requires_custom: bool,
    pub fans: Vec<FanStatus>,
    #[serde(serialize_with = "as_map")]
    pub temps: Vec<(String, f64)>,
}

#[derive(Debug, Serialize)]
pub struct FanStatus {
    pub id: FanId,
    pub label: String,
    pub rpm: u32,
    pub rpm_max: u32,
    pub boost: u8,
    pub boost_pct: u8,
    pub target_boost: u8,
    pub temp_c: Option<f64>,
    pub sensor: String,
}

fn as_map<S: Serializer>(temps: &[(String, f64)], s: S) -> Result<S::Ok, S::Error> {
    s.collect_map(temps.iter().map(|(k, v)| (k, v)))
}

/// Prints the status once, or every second with `watch`. `collect` reads
/// it from the daemon or from sysfs.
pub fn run(
    json: bool,
    watch: bool,
    mut collect: impl FnMut() -> CliResult<Status>,
) -> CliResult<ExitCode> {
    loop {
        let status = collect()?;
        if json {
            let out = if watch {
                serde_json::to_string(&status)
            } else {
                serde_json::to_string_pretty(&status)
            };
            println!(
                "{}",
                out.map_err(|e| Failure::new(Code::Generic, e.to_string()))?
            );
        } else {
            if watch {
                print!("\x1b[2J\x1b[H");
            }
            print!("{}", render(&status));
        }
        if !watch {
            return Ok(match status.health {
                Health::NoDriver => ExitCode::from(Code::NoHardware as u8),
                _ => ExitCode::SUCCESS,
            });
        }
        thread::sleep(Duration::from_secs(1));
    }
}

/// Reads the state straight from sysfs, for when the daemon is stopped.
pub fn collect_direct(ctx: &Ctx) -> Status {
    let (config, config_error) = match ctx.config() {
        Ok(file) => (file.config().clone(), None),
        Err(f) => (Config::default(), Some(f.message)),
    };
    let mut status = Status {
        daemon: false,
        health: Health::Ok,
        health_message: String::new(),
        power_source: ctx
            .root
            .power_source()
            .ok()
            .flatten()
            .unwrap_or(PowerSource::Ac),
        profile: None,
        available_profiles: Vec::new(),
        control: ControlKind::Firmware,
        active_curve: String::new(),
        override_active: false,
        boost_requires_custom: config.hardware.boost_requires_custom,
        fans: Vec::new(),
        temps: ctx.root.read_all_temps(),
    };

    let hw = match Hardware::discover(&ctx.root) {
        Ok(hw) => hw,
        Err(e) => {
            status.health = Health::NoDriver;
            status.health_message = format!("{e}. Rode `alienfan doctor`.");
            return status;
        }
    };
    status.profile = hw.profile().ok();
    status.available_profiles = hw.choices().unwrap_or_default();
    for fan in FanId::ALL.iter().copied() {
        let Ok(reading) = hw.fan(fan) else { continue };
        let sensor = &config.sensors[fan];
        let temp_c = ctx
            .root
            .resolve_sensor(sensor)
            .and_then(|p| read_temp(&p))
            .ok();
        status.fans.push(FanStatus {
            id: fan,
            label: reading.label,
            rpm: reading.rpm,
            rpm_max: reading.rpm_max,
            boost: reading.boost.0,
            boost_pct: reading.boost.pct(),
            target_boost: reading.boost.0,
            temp_c,
            sensor: sensor.to_string(),
        });
    }
    // Without the daemon there is no curve: any boost is a manual one.
    if status.fans.iter().any(|f| f.boost > 0) {
        status.control = ControlKind::Fixed;
    }

    let limit = config.hardware.emergency_temp_c;
    let writable = [
        hw.profile_path(),
        hw.boost_path(FanId::Cpu),
        hw.boost_path(FanId::Gpu),
    ]
    .iter()
    .all(|p| is_writable(p));
    if status
        .fans
        .iter()
        .any(|f| f.temp_c.is_some_and(|t| t > limit))
    {
        status.health = Health::Emergency;
        status.health_message = format!("Temperatura acima de {limit} °C.");
    } else if !writable {
        status.health = Health::NoPermission;
        status.health_message =
            "Sem permissão de escrita no perfil e no boost. Rode `alienfan doctor`.".into();
    } else if let Some(e) = config_error {
        status.health = Health::Degraded;
        status.health_message = format!("{e}. Usando os padrões embutidos.");
    }
    status
}

pub fn render(s: &Status) -> String {
    let mut out = String::new();
    let profile = s.profile.map_or_else(
        || "desconhecido".to_owned(),
        |p| format!("{} ({p})", p.label()),
    );
    let control = match s.control {
        ControlKind::Curve => format!("Curva {}", s.active_curve),
        other => other.label().to_owned(),
    };
    let daemon = if s.daemon {
        "conectado"
    } else {
        "parado (modo direto)"
    };
    let _ = writeln!(out, "Perfil     {profile}");
    let _ = writeln!(out, "Controle   {control}");
    let _ = writeln!(out, "Energia    {}", s.power_source.label());
    let _ = writeln!(out, "Saúde      {}", s.health);
    if !s.health_message.is_empty() {
        let _ = writeln!(out, "           {}", s.health_message);
    }
    let _ = writeln!(out, "Daemon     {daemon}");

    if !s.fans.is_empty() {
        let _ = writeln!(out, "\n           Temp.     RPM          Boost");
        for f in &s.fans {
            let temp = f
                .temp_c
                .map_or_else(|| "  ?  ".to_owned(), |t| format!("{t:>3.0} °C"));
            let rpm = format!("{}/{}", f.rpm, f.rpm_max);
            let _ = write!(
                out,
                "{:<10} {temp:<9} {rpm:<12} {}%",
                f.id.as_str().to_uppercase(),
                f.boost_pct
            );
            if f.id == FanId::Gpu && f.rpm == 0 && f.boost == 0 {
                let _ = write!(out, "   (parada: normal com a GPU ociosa)");
            }
            out.push('\n');
        }
    }
    if !s.temps.is_empty() {
        let _ = writeln!(out, "\nSensores");
        for (name, temp) in &s.temps {
            let _ = writeln!(out, "  {name:<24} {temp:>5.1} °C");
        }
    }
    out
}
