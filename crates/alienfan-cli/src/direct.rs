//! Commands that act on sysfs directly (daemon stopped).

use std::thread;
use std::time::{Duration, Instant};

use alienfan_core::config::Config;
use alienfan_core::plan::{Target, plan};
use alienfan_core::{
    Boost, Control, ControlKind, FanId, FanPair, Hardware, PowerSource, Preset, PresetConfig,
    Profile, SysfsRoot,
};

use crate::{CliResult, Code, Ctx, Failure};

const DAEMON_STOPPED: &str =
    "aviso: daemon parado; mudança não será mantida pela curva nem na troca de energia";

fn warn_daemon_stopped() {
    eprintln!("{DAEMON_STOPPED}");
}

/// The config for hardware commands: a broken file must not block manual
/// fan control, so it falls back to the shipped defaults.
fn config_or_default(ctx: &Ctx) -> Config {
    ctx.config().map_or_else(
        |f| {
            eprintln!("aviso: {}; usando os padrões embutidos", f.message);
            Config::default()
        },
        |file| file.config().clone(),
    )
}

fn current_boost(hw: &Hardware) -> CliResult<FanPair<Boost>> {
    Ok(FanPair::new(hw.boost(FanId::Cpu)?, hw.boost(FanId::Gpu)?))
}

fn apply_target(hw: &Hardware, target: Target) -> CliResult {
    hw.apply(&plan(&target, hw.profile()?))?;
    Ok(())
}

pub fn boost_text(boost: FanPair<Boost>) -> String {
    format!("CPU {}% · GPU {}%", boost.cpu.pct(), boost.gpu.pct())
}

pub fn profile_list(ctx: &Ctx) -> CliResult {
    let hw = ctx.hardware()?;
    let current = hw.profile().ok();
    for p in hw.choices()? {
        let mark = if Some(p) == current { '*' } else { ' ' };
        println!("{mark} {:<22} {}", p.as_str(), p.label());
    }
    Ok(())
}

pub fn profile_set(ctx: &Ctx, profile: Profile) -> CliResult {
    let hw = ctx.hardware()?;
    // Rewrites the current boost: a profile change may reset it.
    let target = Target {
        profile,
        boost: current_boost(&hw)?,
    };
    apply_target(&hw, target)?;
    println!("Perfil: {} ({profile})", profile.label());
    warn_daemon_stopped();
    Ok(())
}

pub fn boost(ctx: &Ctx, fans: &[FanId], value: Boost) -> CliResult {
    let hw = ctx.hardware()?;
    let config = config_or_default(ctx);
    let mut boost = current_boost(&hw)?;
    for &fan in fans {
        boost[fan] = value;
    }
    let preset = Preset {
        profile: hw.profile()?,
        control: Control::Fixed(boost),
    };
    let target = Target::resolve(&preset, config.hardware.boost_requires_custom, boost);
    apply_target(&hw, target)?;
    println!("Boost: {}", boost_text(boost));
    if target.profile != preset.profile {
        println!(
            "Perfil trocado para {} (o boost exige custom)",
            target.profile.label()
        );
    }
    warn_daemon_stopped();
    Ok(())
}

pub fn control(ctx: &Ctx, kind: ControlKind, curve: Option<&str>) -> CliResult {
    let hw = ctx.hardware()?;
    let config = config_or_default(ctx);
    let boost = match kind {
        ControlKind::Firmware => FanPair::both(Boost::MIN),
        ControlKind::Fixed => current_boost(&hw)?,
        ControlKind::Curve => {
            if let Some(name) = curve {
                config.curve(name)?;
            }
            return Err(Failure::new(
                Code::Daemon,
                "o controle por curva precisa do daemon (alienfand), que não está rodando",
            ));
        }
    };
    let control = if kind == ControlKind::Firmware {
        Control::Firmware
    } else {
        Control::Fixed(boost)
    };
    let preset = Preset {
        profile: hw.profile()?,
        control,
    };
    apply_target(
        &hw,
        Target::resolve(&preset, config.hardware.boost_requires_custom, boost),
    )?;
    match kind {
        ControlKind::Fixed => println!(
            "Controle: Manual ({}). Use `alienfan boost` para mudar.",
            boost_text(boost)
        ),
        _ => println!("Controle: {}", kind.label()),
    }
    warn_daemon_stopped();
    Ok(())
}

pub fn preset_text(p: &PresetConfig) -> String {
    let control = match p.control {
        ControlKind::Firmware => p.control.label().to_owned(),
        ControlKind::Fixed => format!("Manual ({})", boost_text(p.fixed)),
        ControlKind::Curve => format!("Curva {}", p.curve),
    };
    format!("{} ({}) · {control}", p.profile.label(), p.profile)
}

pub fn default_show(ctx: &Ctx) -> CliResult {
    let file = ctx.config()?;
    let config = file.config();
    for source in PowerSource::ALL.iter().copied() {
        println!(
            "{:<12} {}",
            format!("{}:", source.label()),
            preset_text(config.defaults.get(source))
        );
    }
    println!("Override     termina em: {}", config.daemon.override_until);
    Ok(())
}

/// Saves the hardware state as the default of the chosen sources (the
/// current one when neither is chosen).
pub fn default_save(ctx: &Ctx, ac: bool, battery: bool) -> CliResult {
    let hw = ctx.hardware()?;
    let mut file = ctx.config()?;
    let boost = current_boost(&hw)?;
    let control = if boost == FanPair::both(Boost::MIN) {
        Control::Firmware
    } else {
        Control::Fixed(boost)
    };
    let preset = Preset {
        profile: hw.profile()?,
        control,
    };
    let sources: Vec<PowerSource> = match (ac, battery) {
        (false, false) => vec![ctx.root.power_source()?.unwrap_or(PowerSource::Ac)],
        _ => PowerSource::ALL
            .iter()
            .copied()
            .filter(|&s| if s == PowerSource::Ac { ac } else { battery })
            .collect(),
    };
    for &source in &sources {
        let mut stored = file.config().defaults.get(source).clone();
        stored.assign(&preset);
        file.set_default(source, &stored)?;
    }
    file.save(&ctx.config_path)?;
    for source in sources {
        println!(
            "Padrão salvo ({}): {}",
            source.label(),
            preset_text(file.config().defaults.get(source))
        );
    }
    Ok(())
}

pub fn default_reset(ctx: &Ctx) -> CliResult {
    apply_default(ctx, Duration::ZERO)?;
    warn_daemon_stopped();
    Ok(())
}

/// `apply`: the default of the current power source. With `boot`, every
/// failure is only logged: the boot must never be blocked.
pub fn apply(ctx: &Ctx, boot: bool, wait: Duration) -> CliResult {
    match apply_default(ctx, wait) {
        Err(f) if boot => {
            eprintln!("alienfan apply --boot: nada aplicado: {}", f.message);
            Ok(())
        }
        other => other,
    }
}

fn apply_default(ctx: &Ctx, wait: Duration) -> CliResult {
    let hw = wait_for_hardware(&ctx.root, wait)?;
    let file = ctx.config()?;
    let config = file.config();
    let source = ctx.root.power_source()?.unwrap_or(PowerSource::Ac);
    let preset = config.defaults.get(source).preset();
    if let Control::Curve(name) = &preset.control {
        eprintln!("aviso: a curva \"{name}\" só roda com o daemon; aplicando boost 0");
    }
    let target = Target::resolve(
        &preset.for_boot(),
        config.hardware.boost_requires_custom,
        FanPair::default(),
    );
    // What the firmware left, for the boot log (Phase 0, T6/T7).
    let before = format!("{} {}", hw.profile()?, boost_text(current_boost(&hw)?));
    apply_target(&hw, target)?;
    println!(
        "{}: perfil {} ({}), boost {} (antes: {before})",
        source.label(),
        target.profile.label(),
        target.profile,
        boost_text(target.boost)
    );
    Ok(())
}

/// Polls until the driver shows up, for the boot service.
fn wait_for_hardware(root: &SysfsRoot, wait: Duration) -> CliResult<Hardware> {
    let deadline = Instant::now() + wait;
    loop {
        match Hardware::discover(root) {
            Ok(hw) => return Ok(hw),
            Err(e) if Instant::now() >= deadline => return Err(e.into()),
            Err(_) => thread::sleep(Duration::from_millis(250)),
        }
    }
}
