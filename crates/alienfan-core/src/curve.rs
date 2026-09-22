//! Fan curves: temperature → boost (SPEC 6.2 and 6.3).

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::model::{Boost, FanId, FanPair};

pub const MIN_POINTS: usize = 2;
pub const MAX_POINTS: usize = 16;
pub const MAX_TEMP_C: f64 = 105.0;
pub const MAX_HYSTERESIS_C: f64 = 15.0;
pub const MAX_NAME_LEN: usize = 32;
pub const DEFAULT_HYSTERESIS_C: f64 = 3.0;
pub const DEFAULT_RAMP_UP_PER_S: u16 = 40;
pub const DEFAULT_RAMP_DOWN_PER_S: u16 = 10;

/// One curve point. In TOML it is written `[temp_c, boost]`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(from = "(f64, Boost)", into = "(f64, Boost)")]
pub struct CurvePoint {
    pub temp_c: f64,
    pub boost: Boost,
}

impl CurvePoint {
    pub const fn new(temp_c: f64, boost: u8) -> Self {
        Self {
            temp_c,
            boost: Boost(boost),
        }
    }
}

impl From<(f64, Boost)> for CurvePoint {
    fn from((temp_c, boost): (f64, Boost)) -> Self {
        Self { temp_c, boost }
    }
}

impl From<CurvePoint> for (f64, Boost) {
    fn from(p: CurvePoint) -> Self {
        (p.temp_c, p.boost)
    }
}

/// A validated curve. Deserializing checks every rule of SPEC 6.2, so a
/// `Curve` value is always usable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "CurveDef")]
pub struct Curve {
    pub cpu: Vec<CurvePoint>,
    pub gpu: Vec<CurvePoint>,
    pub hysteresis_c: f64,
    /// Boost units per second.
    pub ramp_up_per_s: u16,
    /// Boost units per second.
    pub ramp_down_per_s: u16,
}

/// Unvalidated mirror of [`Curve`], with the SPEC defaults.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CurveDef {
    cpu: Vec<CurvePoint>,
    gpu: Vec<CurvePoint>,
    #[serde(default = "default_hysteresis")]
    hysteresis_c: f64,
    #[serde(default = "default_ramp_up")]
    ramp_up_per_s: u16,
    #[serde(default = "default_ramp_down")]
    ramp_down_per_s: u16,
}

const fn default_hysteresis() -> f64 {
    DEFAULT_HYSTERESIS_C
}
const fn default_ramp_up() -> u16 {
    DEFAULT_RAMP_UP_PER_S
}
const fn default_ramp_down() -> u16 {
    DEFAULT_RAMP_DOWN_PER_S
}

impl TryFrom<CurveDef> for Curve {
    type Error = Error;

    fn try_from(d: CurveDef) -> Result<Self> {
        Self::new(
            FanPair::new(d.cpu, d.gpu),
            d.hysteresis_c,
            d.ramp_up_per_s,
            d.ramp_down_per_s,
        )
    }
}

impl Curve {
    /// Builds a curve, checking every rule of SPEC 6.2.
    pub fn new(
        points: FanPair<Vec<CurvePoint>>,
        hysteresis_c: f64,
        ramp_up_per_s: u16,
        ramp_down_per_s: u16,
    ) -> Result<Self> {
        let curve = Self {
            cpu: points.cpu,
            gpu: points.gpu,
            hysteresis_c,
            ramp_up_per_s,
            ramp_down_per_s,
        };
        curve.validate()?;
        Ok(curve)
    }

    pub fn points(&self, fan: FanId) -> &[CurvePoint] {
        match fan {
            FanId::Cpu => &self.cpu,
            FanId::Gpu => &self.gpu,
        }
    }

    pub fn validate(&self) -> Result<()> {
        for fan in FanId::ALL.iter().copied() {
            validate_points(self.points(fan))
                .map_err(|e| Error::invalid(format!("curva da {fan}: {e}")))?;
        }
        if !(0.0..=MAX_HYSTERESIS_C).contains(&self.hysteresis_c) {
            return Err(Error::invalid(format!(
                "histerese fora do intervalo 0–{MAX_HYSTERESIS_C} °C: {}",
                self.hysteresis_c
            )));
        }
        for (name, ramp) in [
            ("ramp_up_per_s", self.ramp_up_per_s),
            ("ramp_down_per_s", self.ramp_down_per_s),
        ] {
            if !(1..=255).contains(&ramp) {
                return Err(Error::invalid(format!(
                    "{name} fora do intervalo 1–255: {ramp}"
                )));
            }
        }
        Ok(())
    }

    /// Target boost for `fan` at `temp_c`.
    pub fn target(&self, fan: FanId, temp_c: f64) -> Boost {
        interpolate(self.points(fan), temp_c)
    }
}

fn validate_points(points: &[CurvePoint]) -> Result<(), String> {
    if !(MIN_POINTS..=MAX_POINTS).contains(&points.len()) {
        return Err(format!(
            "precisa de {MIN_POINTS} a {MAX_POINTS} pontos, tem {}",
            points.len()
        ));
    }
    for p in points {
        if !(0.0..=MAX_TEMP_C).contains(&p.temp_c) {
            return Err(format!(
                "temperatura fora do intervalo 0–{MAX_TEMP_C} °C: {}",
                p.temp_c
            ));
        }
    }
    for w in points.windows(2) {
        if w[1].temp_c <= w[0].temp_c {
            return Err(format!(
                "as temperaturas precisam ser estritamente crescentes ({} depois de {})",
                w[1].temp_c, w[0].temp_c
            ));
        }
        if w[1].boost < w[0].boost {
            return Err(format!(
                "o boost não pode cair quando a temperatura sobe ({} °C → {}, {} °C → {})",
                w[0].temp_c, w[0].boost, w[1].temp_c, w[1].boost
            ));
        }
    }
    Ok(())
}

/// Piecewise-linear interpolation, flat outside the points, rounded.
///
/// `points` must be valid (see [`Curve::validate`]).
pub fn interpolate(points: &[CurvePoint], temp_c: f64) -> Boost {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return Boost::MIN;
    };
    if temp_c <= first.temp_c {
        return first.boost;
    }
    if temp_c >= last.temp_c {
        return last.boost;
    }
    let (a, b) = points
        .windows(2)
        .map(|w| (w[0], w[1]))
        .find(|(_, b)| temp_c <= b.temp_c)
        .unwrap_or((*last, *last));
    let f = (temp_c - a.temp_c) / (b.temp_c - a.temp_c);
    let (ba, bb) = (f64::from(a.boost.0), f64::from(b.boost.0));
    Boost::from_f64(ba + f * (bb - ba))
}

/// Curve names become TOML keys, D-Bus strings and CLI arguments.
pub fn validate_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.chars().count() <= MAX_NAME_LEN
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(Error::invalid(format!(
            "nome de curva inválido: \"{name}\" (use até {MAX_NAME_LEN} letras, números, - ou _)"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pts(list: &[(f64, u8)]) -> Vec<CurvePoint> {
        list.iter().map(|&(t, b)| CurvePoint::new(t, b)).collect()
    }

    fn curve(list: &[(f64, u8)]) -> Result<Curve> {
        Curve::new(FanPair::both(pts(list)), 3.0, 40, 10)
    }

    const EQUILIBRADO: &[(f64, u8)] =
        &[(45.0, 0), (60.0, 51), (70.0, 115), (80.0, 179), (90.0, 255)];

    #[test]
    fn valid_curve_passes() {
        curve(EQUILIBRADO).unwrap();
    }

    #[test]
    fn too_few_points() {
        let err = curve(&[(50.0, 0)]).unwrap_err().to_string();
        assert!(err.contains("2 a 16"), "{err}");
    }

    #[test]
    fn too_many_points() {
        let many: Vec<_> = (0..17u8).map(|i| (f64::from(i) * 5.0, i)).collect();
        assert!(curve(&many).is_err());
    }

    #[test]
    fn temp_out_of_range() {
        assert!(curve(&[(-1.0, 0), (50.0, 10)]).is_err());
        assert!(curve(&[(50.0, 0), (106.0, 10)]).is_err());
        curve(&[(0.0, 0), (105.0, 10)]).unwrap();
    }

    #[test]
    fn temps_must_strictly_increase() {
        let err = curve(&[(50.0, 0), (50.0, 10)]).unwrap_err().to_string();
        assert!(err.contains("estritamente crescentes"), "{err}");
        assert!(curve(&[(60.0, 0), (50.0, 10)]).is_err());
    }

    #[test]
    fn boost_must_not_decrease() {
        let err = curve(&[(50.0, 100), (60.0, 50)]).unwrap_err().to_string();
        assert!(err.contains("não pode cair"), "{err}");
        curve(&[(50.0, 100), (60.0, 100)]).unwrap();
    }

    #[test]
    fn error_names_the_fan() {
        let err = Curve::new(
            FanPair::new(pts(EQUILIBRADO), pts(&[(1.0, 0)])),
            3.0,
            40,
            10,
        )
        .unwrap_err()
        .to_string();
        assert!(err.starts_with("curva da gpu"), "{err}");
    }

    #[test]
    fn hysteresis_and_ramp_ranges() {
        let p = FanPair::both(pts(EQUILIBRADO));
        assert!(Curve::new(p.clone(), -0.1, 40, 10).is_err());
        assert!(Curve::new(p.clone(), 15.1, 40, 10).is_err());
        assert!(Curve::new(p.clone(), 3.0, 0, 10).is_err());
        assert!(Curve::new(p.clone(), 3.0, 40, 256).is_err());
        Curve::new(p.clone(), 0.0, 1, 1).unwrap();
        Curve::new(p, 15.0, 255, 255).unwrap();
    }

    #[test]
    fn interpolation_below_between_above() {
        let p = pts(EQUILIBRADO);
        assert_eq!(interpolate(&p, 20.0), Boost(0));
        assert_eq!(interpolate(&p, 45.0), Boost(0));
        assert_eq!(interpolate(&p, 52.5), Boost(26)); // 25.5 rounds up
        assert_eq!(interpolate(&p, 60.0), Boost(51));
        assert_eq!(interpolate(&p, 75.0), Boost(147));
        assert_eq!(interpolate(&p, 90.0), Boost(255));
        assert_eq!(interpolate(&p, 104.0), Boost(255));
    }

    #[test]
    fn names() {
        validate_name("equilibrado").unwrap();
        validate_name("meu-perfil_2").unwrap();
        validate_name("rápido").unwrap();
        assert!(validate_name("").is_err());
        assert!(validate_name("com espaço").is_err());
        assert!(validate_name("a.b").is_err());
        assert!(validate_name(&"x".repeat(33)).is_err());
    }
}
