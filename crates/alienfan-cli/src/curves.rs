//! `alienfan curve …`: curves live in the config, so these commands never
//! touch the hardware.

use alienfan_core::curve::{DEFAULT_HYSTERESIS_C, DEFAULT_RAMP_DOWN_PER_S, DEFAULT_RAMP_UP_PER_S};
use alienfan_core::{Boost, Curve, CurvePoint, Error, FanId, FanPair, PowerSource};
use clap::Args;

use crate::{CliResult, Ctx};

#[derive(Args)]
pub struct SetArgs {
    pub name: String,
    /// Pontos da CPU: "temp:boost,…", boost bruto ou com %, ex. "45:0,70:45%,90:100%"
    #[arg(long, value_name = "PONTOS", value_parser = parse_points)]
    pub cpu: Points,
    /// Pontos da GPU (padrão: os atuais da curva, ou os da CPU numa curva nova)
    #[arg(long, value_name = "PONTOS", value_parser = parse_points)]
    pub gpu: Option<Points>,
    /// Histerese em °C
    #[arg(long, value_name = "°C")]
    pub hysteresis: Option<f64>,
    /// Subida máxima, em unidades de boost por segundo
    #[arg(long, value_name = "N")]
    pub ramp_up: Option<u16>,
    /// Descida máxima, em unidades de boost por segundo
    #[arg(long, value_name = "N")]
    pub ramp_down: Option<u16>,
}

/// A whole point list as one argument (clap would split a bare `Vec`).
#[derive(Debug, Clone, PartialEq)]
pub struct Points(pub Vec<CurvePoint>);

/// Parses `"45:0,60:20%,90:100%"`.
pub fn parse_points(s: &str) -> Result<Points, Error> {
    s.split(',')
        .map(|point| {
            let bad = || Error::Invalid(format!("ponto inválido: \"{point}\" (use temp:boost)"));
            let (temp, boost) = point.split_once(':').ok_or_else(bad)?;
            Ok(CurvePoint {
                temp_c: temp.trim().parse().map_err(|_| bad())?,
                boost: boost.parse::<Boost>()?,
            })
        })
        .collect::<Result<_, _>>()
        .map(Points)
}

fn points_text(points: &[CurvePoint]) -> String {
    points
        .iter()
        .map(|p| format!("{} °C → {}%", p.temp_c, p.boost.pct()))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn list(ctx: &Ctx) -> CliResult {
    let file = ctx.config()?;
    let config = file.config();
    let entries: Vec<_> = config
        .curves
        .keys()
        .map(|name| (name.clone(), config.curve_users(name)))
        .collect();
    print_list(&entries);
    Ok(())
}

/// Curve names, each with the defaults that run it.
pub fn print_list(entries: &[(String, Vec<PowerSource>)]) {
    for (name, users) in entries {
        if users.is_empty() {
            println!("{name}");
        } else {
            let users: Vec<_> = users.iter().map(|s| s.label().to_lowercase()).collect();
            println!("{name:<16} (padrão: {})", users.join(", "));
        }
    }
}

pub fn show(ctx: &Ctx, name: &str) -> CliResult {
    let file = ctx.config()?;
    print_curve(name, file.config().curve(name)?);
    Ok(())
}

pub fn print_curve(name: &str, curve: &Curve) {
    println!("Curva \"{name}\"");
    for fan in FanId::ALL.iter().copied() {
        println!(
            "  {}: {}",
            fan.as_str().to_uppercase(),
            points_text(curve.points(fan))
        );
    }
    println!(
        "  Histerese {} °C · subida {}/s · descida {}/s",
        curve.hysteresis_c, curve.ramp_up_per_s, curve.ramp_down_per_s
    );
}

pub fn set(ctx: &Ctx, args: &SetArgs) -> CliResult {
    let mut file = ctx.config()?;
    let existing = file.config().curves.get(&args.name);
    let cpu = args.cpu.0.clone();
    let gpu = args
        .gpu
        .as_ref()
        .map(|p| p.0.clone())
        .or_else(|| existing.map(|c| c.gpu.clone()))
        .unwrap_or_else(|| cpu.clone());
    let curve = Curve::new(
        FanPair::new(cpu, gpu),
        args.hysteresis
            .or(existing.map(|c| c.hysteresis_c))
            .unwrap_or(DEFAULT_HYSTERESIS_C),
        args.ramp_up
            .or(existing.map(|c| c.ramp_up_per_s))
            .unwrap_or(DEFAULT_RAMP_UP_PER_S),
        args.ramp_down
            .or(existing.map(|c| c.ramp_down_per_s))
            .unwrap_or(DEFAULT_RAMP_DOWN_PER_S),
    )?;
    let created = existing.is_none();
    file.set_curve(&args.name, &curve)?;
    file.save(&ctx.config_path)?;
    let verb = if created { "criada" } else { "atualizada" };
    println!("Curva \"{}\" {verb}.", args.name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_accept_raw_and_pct() {
        let Points(p) = parse_points("45:0, 60:20%,90.5:255").unwrap();
        assert_eq!(
            p,
            [
                CurvePoint::new(45.0, 0),
                CurvePoint::new(60.0, 51),
                CurvePoint::new(90.5, 255)
            ]
        );
    }

    #[test]
    fn bad_points() {
        for bad in [
            "",
            "45",
            "45:",
            ":10",
            "x:10",
            "45:300",
            "45:101%",
            "45:0;60:10",
        ] {
            assert!(parse_points(bad).is_err(), "{bad:?}");
        }
    }
}
