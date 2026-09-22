//! Domain types (SPEC 6.1).

use std::fmt;
use std::ops::{Index, IndexMut};
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{Error, Result};

/// Declares a fieldless enum whose values are fixed strings: the same string
/// is used in sysfs, the config, D-Bus and the CLI.
macro_rules! str_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident ($what:literal) {
            $($(#[$vmeta:meta])* $variant:ident = $s:literal,)+
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name {
            $($(#[$vmeta])* $variant,)+
        }

        impl $name {
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];

            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $s,)+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = Error;

            fn from_str(s: &str) -> Result<Self> {
                Self::ALL
                    .iter()
                    .copied()
                    .find(|v| v.as_str() == s)
                    .ok_or_else(|| {
                        let valid: Vec<_> = Self::ALL.iter().map(|v| v.as_str()).collect();
                        Error::invalid(format!(
                            "valor inválido para {}: \"{s}\" (use {})",
                            $what,
                            valid.join(", ")
                        ))
                    })
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let s = String::deserialize(deserializer)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

str_enum! {
    /// Thermal profile, spelled exactly as the kernel's platform-profile class.
    ///
    /// Which profiles exist comes from `choices` at runtime, never from `ALL`.
    pub enum Profile ("perfil") {
        Cool = "cool",
        Quiet = "quiet",
        Balanced = "balanced",
        BalancedPerformance = "balanced-performance",
        /// Also turns on G-Mode on this model.
        Performance = "performance",
        Custom = "custom",
    }
}

impl Profile {
    /// Parses a sysfs `choices` line, skipping names this build does not know.
    pub fn parse_choices(line: &str) -> Vec<Self> {
        line.split_whitespace()
            .filter_map(|s| s.parse().ok())
            .collect()
    }
}

str_enum! {
    /// `fan1` is the CPU fan and `fan2` the GPU fan (checked against the labels).
    pub enum FanId ("ventoinha") {
        Cpu = "cpu",
        Gpu = "gpu",
    }
}

impl FanId {
    /// Index `N` of the `fanN_*` and `tempN_*` sysfs files.
    pub const fn sysfs_index(self) -> u8 {
        match self {
            Self::Cpu => 1,
            Self::Gpu => 2,
        }
    }
}

str_enum! {
    pub enum PowerSource ("fonte de energia") {
        Ac = "ac",
        Battery = "battery",
    }
}

str_enum! {
    pub enum ControlKind ("controle") {
        /// Boost 0 on both fans: the firmware curve alone.
        Firmware = "firmware",
        Fixed = "fixed",
        Curve = "curve",
    }
}

str_enum! {
    pub enum Health ("saúde") {
        Ok = "ok",
        Degraded = "degraded",
        Emergency = "emergency",
        NoPermission = "no-permission",
        NoDriver = "no-driver",
    }
}

str_enum! {
    /// When a manual override ends (besides `RestoreDefault` and a restart).
    pub enum OverrideUntil ("override_until") {
        PowerChange = "power-change",
        Manual = "manual",
    }
}

/// One value per fan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FanPair<T> {
    pub cpu: T,
    pub gpu: T,
}

impl<T> FanPair<T> {
    pub const fn new(cpu: T, gpu: T) -> Self {
        Self { cpu, gpu }
    }

    pub fn both(value: T) -> Self
    where
        T: Clone,
    {
        Self::new(value.clone(), value)
    }

    pub fn map<U>(self, mut f: impl FnMut(FanId, T) -> U) -> FanPair<U> {
        FanPair::new(f(FanId::Cpu, self.cpu), f(FanId::Gpu, self.gpu))
    }

    pub fn iter(&self) -> impl Iterator<Item = (FanId, &T)> {
        [(FanId::Cpu, &self.cpu), (FanId::Gpu, &self.gpu)].into_iter()
    }
}

impl<T> Index<FanId> for FanPair<T> {
    type Output = T;

    fn index(&self, fan: FanId) -> &T {
        match fan {
            FanId::Cpu => &self.cpu,
            FanId::Gpu => &self.gpu,
        }
    }
}

impl<T> IndexMut<FanId> for FanPair<T> {
    fn index_mut(&mut self, fan: FanId) -> &mut T {
        match fan {
            FanId::Cpu => &mut self.cpu,
            FanId::Gpu => &mut self.gpu,
        }
    }
}

/// Fan boost in hardware units, 0–255. It only adds speed on top of the
/// firmware base (SPEC 3.1).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Boost(pub u8);

impl Boost {
    pub const MIN: Self = Self(0);
    pub const MAX: Self = Self(u8::MAX);

    /// Percentage shown by the UI and CLI: `round(boost * 100 / 255)`.
    #[allow(clippy::cast_possible_truncation)] // at most 100
    pub fn pct(self) -> u8 {
        ((u16::from(self.0) * 100 + 127) / 255) as u8
    }

    /// Inverse of [`Boost::pct`]: `round(pct * 255 / 100)`.
    #[allow(clippy::cast_possible_truncation)] // at most 255
    pub fn from_pct(pct: u8) -> Result<Self> {
        if pct > 100 {
            return Err(Error::invalid(format!(
                "porcentagem fora do intervalo 0–100: {pct}"
            )));
        }
        Ok(Self(((u16::from(pct) * 255 + 50) / 100) as u8))
    }

    /// Rounds and clamps a fractional boost, as computed by the curve ramps.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped first
    pub fn from_f64(value: f64) -> Self {
        Self(value.round().clamp(0.0, 255.0) as u8)
    }
}

impl fmt::Display for Boost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// CLI syntax: raw `0`–`255`, or a percentage `0%`–`100%`.
impl FromStr for Boost {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim();
        let bad = || Error::invalid(format!("boost inválido: \"{s}\" (use 0–255 ou 0–100%)"));
        match s.strip_suffix('%') {
            Some(pct) => Self::from_pct(pct.trim().parse().map_err(|_| bad())?),
            None => s.parse().map(Self).map_err(|_| bad()),
        }
    }
}

/// How the fan boost is chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Control {
    Firmware,
    Fixed(FanPair<Boost>),
    /// Name of a `[curves.<name>]` table.
    Curve(String),
}

impl Control {
    pub fn kind(&self) -> ControlKind {
        match self {
            Self::Firmware => ControlKind::Firmware,
            Self::Fixed(_) => ControlKind::Fixed,
            Self::Curve(_) => ControlKind::Curve,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    pub profile: Profile,
    pub control: Control,
}

impl Preset {
    /// The boot service runs before the daemon, so it cannot follow a curve:
    /// it applies the profile with boost 0 instead (SPEC 10, `apply --boot`).
    #[must_use]
    pub fn for_boot(&self) -> Self {
        let control = match &self.control {
            Control::Curve(_) => Control::Firmware,
            other => other.clone(),
        };
        Self {
            profile: self.profile,
            control,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_names_round_trip() {
        for &p in Profile::ALL {
            assert_eq!(p.as_str().parse::<Profile>().unwrap(), p);
        }
        assert_eq!(
            "balanced-performance".parse::<Profile>().unwrap(),
            Profile::BalancedPerformance
        );
        let err = "turbo".parse::<Profile>().unwrap_err().to_string();
        assert!(err.contains("perfil") && err.contains("turbo"), "{err}");
    }

    #[test]
    fn choices_skip_unknown_names() {
        let choices = Profile::parse_choices("low-power cool balanced custom\n");
        assert_eq!(choices, [Profile::Cool, Profile::Balanced, Profile::Custom]);
    }

    #[test]
    fn pct_conversion_rounds() {
        assert_eq!(Boost(0).pct(), 0);
        assert_eq!(Boost(255).pct(), 100);
        assert_eq!(Boost(128).pct(), 50);
        assert_eq!(Boost(1).pct(), 0);
        assert_eq!(Boost(2).pct(), 1);
        assert_eq!(Boost::from_pct(0).unwrap(), Boost(0));
        assert_eq!(Boost::from_pct(10).unwrap(), Boost(26)); // 25.5 rounds up
        assert_eq!(Boost::from_pct(60).unwrap(), Boost(153));
        assert_eq!(Boost::from_pct(100).unwrap(), Boost(255));
        assert!(Boost::from_pct(101).is_err());
    }

    #[test]
    fn pct_round_trip_is_stable() {
        for pct in 0..=100 {
            assert_eq!(Boost::from_pct(pct).unwrap().pct(), pct);
        }
    }

    #[test]
    fn boost_parses_raw_and_pct() {
        assert_eq!("0".parse::<Boost>().unwrap(), Boost(0));
        assert_eq!("255".parse::<Boost>().unwrap(), Boost(255));
        assert_eq!("60%".parse::<Boost>().unwrap(), Boost(153));
        assert!("256".parse::<Boost>().is_err());
        assert!("-1".parse::<Boost>().is_err());
        assert!("101%".parse::<Boost>().is_err());
        assert!("abc".parse::<Boost>().is_err());
    }

    #[test]
    fn boot_preset_drops_the_curve() {
        let preset = Preset {
            profile: Profile::Quiet,
            control: Control::Curve("silencioso".into()),
        };
        assert_eq!(preset.for_boot().control, Control::Firmware);
        assert_eq!(preset.for_boot().profile, Profile::Quiet);

        let fixed = Preset {
            profile: Profile::Quiet,
            control: Control::Fixed(FanPair::both(Boost(10))),
        };
        assert_eq!(fixed.for_boot(), fixed);
    }
}
