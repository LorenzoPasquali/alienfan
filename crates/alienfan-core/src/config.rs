//! `/etc/alienfan/config.toml` (SPEC 7).
//!
//! [`Config`] is the parsed, validated view. [`ConfigFile`] keeps the TOML
//! document next to it, so edits made by the daemon keep the user's comments.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File, Permissions};
use std::io::{self, Write as _};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value};

use crate::curve::{self, Curve};
use crate::error::{ConfigError, Error, Result};
use crate::model::{
    Boost, Control, ControlKind, FanId, FanPair, OverrideUntil, PowerSource, Preset, Profile,
};

pub const CONFIG_PATH: &str = "/etc/alienfan/config.toml";
/// Overrides [`CONFIG_PATH`] in tests.
pub const CONFIG_ENV: &str = "ALIENFAN_CONFIG";
pub const CONFIG_VERSION: u32 = 1;
pub const CONFIG_MODE: u32 = 0o664;

/// The shipped config, used when the file does not exist.
pub const DEFAULT_CONFIG: &str = include_str!("../../../packaging/config/config.toml");

pub const TICK_MS_RANGE: std::ops::RangeInclusive<u32> = 250..=5000;
pub const EMERGENCY_TEMP_RANGE: std::ops::RangeInclusive<f64> = 60.0..=105.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub hardware: HardwareConfig,
    #[serde(default)]
    pub daemon: DaemonConfig,
    #[serde(default = "default_sensors")]
    pub sensors: FanPair<SensorRef>,
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub curves: BTreeMap<String, Curve>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HardwareConfig {
    /// Result of Phase 0: fan boost only works in the `custom` profile.
    pub boost_requires_custom: bool,
    /// At or above this, both fans go to boost 255.
    pub emergency_temp_c: f64,
}

impl Default for HardwareConfig {
    fn default() -> Self {
        Self {
            boost_requires_custom: false,
            emergency_temp_c: 95.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DaemonConfig {
    pub tick_ms: u32,
    pub override_until: OverrideUntil,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            tick_ms: 1000,
            override_until: OverrideUntil::PowerChange,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Defaults {
    pub ac: PresetConfig,
    pub battery: PresetConfig,
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            ac: PresetConfig {
                profile: Profile::BalancedPerformance,
                control: ControlKind::Firmware,
                fixed: FanPair::default(),
                curve: "equilibrado".into(),
            },
            battery: PresetConfig {
                profile: Profile::Balanced,
                control: ControlKind::Firmware,
                fixed: FanPair::default(),
                curve: "silencioso".into(),
            },
        }
    }
}

impl Defaults {
    pub fn get(&self, source: PowerSource) -> &PresetConfig {
        match source {
            PowerSource::Ac => &self.ac,
            PowerSource::Battery => &self.battery,
        }
    }

    pub fn get_mut(&mut self, source: PowerSource) -> &mut PresetConfig {
        match source {
            PowerSource::Ac => &mut self.ac,
            PowerSource::Battery => &mut self.battery,
        }
    }
}

/// A default as stored: it keeps the fixed boost and the curve name even
/// while another control is selected, so the editors can show them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresetConfig {
    pub profile: Profile,
    pub control: ControlKind,
    #[serde(default)]
    pub fixed: FanPair<Boost>,
    #[serde(default)]
    pub curve: String,
}

impl PresetConfig {
    pub fn preset(&self) -> Preset {
        let control = match self.control {
            ControlKind::Firmware => Control::Firmware,
            ControlKind::Fixed => Control::Fixed(self.fixed),
            ControlKind::Curve => Control::Curve(self.curve.clone()),
        };
        Preset {
            profile: self.profile,
            control,
        }
    }

    /// Stores `preset`, keeping the fields that its control does not use.
    pub fn assign(&mut self, preset: &Preset) {
        self.profile = preset.profile;
        self.control = preset.control.kind();
        match &preset.control {
            Control::Firmware => {}
            Control::Fixed(boost) => self.fixed = *boost,
            Control::Curve(name) => self.curve.clone_from(name),
        }
    }
}

/// A temperature sensor: `"<hwmon name>:<label>"` or `"<hwmon name>#<index>"`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SensorRef {
    pub hwmon: String,
    pub selector: SensorSelector,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SensorSelector {
    /// Matches `tempN_label`.
    Label(String),
    /// The `N` of `tempN_input`, for hwmons with repeated labels.
    Index(u32),
}

impl SensorRef {
    pub fn label(hwmon: &str, label: &str) -> Self {
        Self {
            hwmon: hwmon.into(),
            selector: SensorSelector::Label(label.into()),
        }
    }
}

impl fmt::Display for SensorRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.selector {
            SensorSelector::Label(label) => write!(f, "{}:{label}", self.hwmon),
            SensorSelector::Index(index) => write!(f, "{}#{index}", self.hwmon),
        }
    }
}

impl FromStr for SensorRef {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        let bad = || {
            Error::invalid(format!(
                "sensor inválido: \"{s}\" (use \"<hwmon>:<label>\" ou \"<hwmon>#<índice>\")"
            ))
        };
        let at = s.find([':', '#']).ok_or_else(bad)?;
        let (hwmon, rest) = s.split_at(at);
        let (sep, value) = rest.split_at(1);
        if hwmon.is_empty() || value.is_empty() {
            return Err(bad());
        }
        let selector = if sep == "#" {
            SensorSelector::Index(value.parse().map_err(|_| bad())?)
        } else {
            SensorSelector::Label(value.into())
        };
        Ok(Self {
            hwmon: hwmon.into(),
            selector,
        })
    }
}

impl Serialize for SensorRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for SensorRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// `$ALIENFAN_CONFIG`, or [`CONFIG_PATH`].
pub fn config_path() -> PathBuf {
    std::env::var_os(CONFIG_ENV).map_or_else(|| CONFIG_PATH.into(), PathBuf::from)
}

const fn default_version() -> u32 {
    CONFIG_VERSION
}

fn default_sensors() -> FanPair<SensorRef> {
    FanPair::new(
        SensorRef::label("alienware_wmi", "CPU"),
        SensorRef::label("alienware_wmi", "GPU"),
    )
}

impl Default for Config {
    fn default() -> Self {
        Self::parse(DEFAULT_CONFIG).expect("the shipped config is valid")
    }
}

impl Config {
    /// Parses and validates. Syntax and type errors carry line and column.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let config: Self = toml::from_str(text).map_err(|e| match e.span() {
            Some(span) => ConfigError::at(e.message(), text, span.start),
            None => ConfigError::new(e.message()),
        })?;
        config.validate().map_err(ConfigError::new)?;
        Ok(config)
    }

    /// Rules that span several tables. Per-curve rules run while parsing.
    pub fn validate(&self) -> Result<(), String> {
        if self.version != CONFIG_VERSION {
            return Err(format!(
                "version = {} não é suportada (esperado {CONFIG_VERSION})",
                self.version
            ));
        }
        if !EMERGENCY_TEMP_RANGE.contains(&self.hardware.emergency_temp_c) {
            return Err(format!(
                "hardware.emergency_temp_c fora do intervalo {}–{} °C: {}",
                EMERGENCY_TEMP_RANGE.start(),
                EMERGENCY_TEMP_RANGE.end(),
                self.hardware.emergency_temp_c
            ));
        }
        if !TICK_MS_RANGE.contains(&self.daemon.tick_ms) {
            return Err(format!(
                "daemon.tick_ms fora do intervalo {}–{}: {}",
                TICK_MS_RANGE.start(),
                TICK_MS_RANGE.end(),
                self.daemon.tick_ms
            ));
        }
        for (name, curve) in &self.curves {
            curve::validate_name(name).map_err(|e| e.to_string())?;
            curve
                .validate()
                .map_err(|e| format!("curves.{name}: {e}"))?;
        }
        for source in PowerSource::ALL.iter().copied() {
            let preset = self.defaults.get(source);
            if preset.control == ControlKind::Curve && !self.curves.contains_key(&preset.curve) {
                return Err(format!(
                    "defaults.{source}.curve = \"{}\" não existe em [curves]",
                    preset.curve
                ));
            }
        }
        Ok(())
    }

    pub fn curve(&self, name: &str) -> Result<&Curve> {
        self.curves
            .get(name)
            .ok_or_else(|| Error::invalid(format!("curva \"{name}\" não existe")))
    }

    /// Where a curve is the active control of a default, e.g. `"ac"`.
    pub fn curve_users(&self, name: &str) -> Vec<PowerSource> {
        PowerSource::ALL
            .iter()
            .copied()
            .filter(|&s| {
                let p = self.defaults.get(s);
                p.control == ControlKind::Curve && p.curve == name
            })
            .collect()
    }
}

/// The config file as a TOML document plus its parsed view.
///
/// Every edit goes through the document and is parsed back, so `config()`
/// always matches what `text()` would write.
#[derive(Debug, Clone)]
pub struct ConfigFile {
    doc: DocumentMut,
    config: Config,
}

impl Default for ConfigFile {
    fn default() -> Self {
        Self::parse(DEFAULT_CONFIG).expect("the shipped config is valid")
    }
}

impl ConfigFile {
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let config = Config::parse(text)?;
        let doc = text
            .parse::<DocumentMut>()
            .map_err(|e| ConfigError::new(e.to_string()))?;
        Ok(Self { doc, config })
    }

    /// Reads `path`. A missing file gives the shipped defaults.
    pub fn load(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => Ok(Self::parse(&text)?),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(Error::from_io(path, e)),
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn text(&self) -> String {
        self.doc.to_string()
    }

    /// Writes the file atomically, keeping the previous one as `.bak`.
    pub fn save(&self, path: &Path) -> Result<()> {
        write_atomic(path, &self.text())
    }

    pub fn set_default(&mut self, source: PowerSource, preset: &PresetConfig) -> Result<()> {
        self.edit(|doc| {
            let defaults = table(doc.as_table_mut(), "defaults");
            let t = table(defaults, source.as_str());
            set_value(t, "profile", preset.profile.as_str().into());
            set_value(t, "control", preset.control.as_str().into());
            set_value(t, "fixed", fan_pair_value(preset.fixed).into());
            set_value(t, "curve", preset.curve.as_str().into());
        })
    }

    /// Creates or replaces `[curves.<name>]`.
    pub fn set_curve(&mut self, name: &str, curve: &Curve) -> Result<()> {
        curve::validate_name(name)?;
        curve.validate()?;
        self.edit(|doc| {
            let curves = table(doc.as_table_mut(), "curves");
            let t = table(curves, name);
            for fan in FanId::ALL.iter().copied() {
                set_value(t, fan.as_str(), points_value(curve, fan).into());
            }
            set_value(t, "hysteresis_c", curve.hysteresis_c.into());
            set_value(t, "ramp_up_per_s", i64::from(curve.ramp_up_per_s).into());
            set_value(
                t,
                "ramp_down_per_s",
                i64::from(curve.ramp_down_per_s).into(),
            );
        })
    }

    /// Removes `[curves.<name>]`. Fails if a default uses it.
    pub fn remove_curve(&mut self, name: &str) -> Result<()> {
        self.config.curve(name)?;
        if let Some(source) = self.config.curve_users(name).first() {
            return Err(Error::CurveInUse {
                name: name.into(),
                used_by: power_label(*source).into(),
            });
        }
        self.edit(|doc| {
            if let Some(curves) = doc.get_mut("curves").and_then(Item::as_table_like_mut) {
                curves.remove(name);
            }
        })
    }

    pub fn set_override_until(&mut self, value: OverrideUntil) -> Result<()> {
        self.edit(|doc| {
            let daemon = table(doc.as_table_mut(), "daemon");
            set_value(daemon, "override_until", value.as_str().into());
        })
    }

    fn edit(&mut self, f: impl FnOnce(&mut DocumentMut)) -> Result<()> {
        let mut doc = self.doc.clone();
        f(&mut doc);
        self.config = Config::parse(&doc.to_string())?;
        self.doc = doc;
        Ok(())
    }
}

fn power_label(source: PowerSource) -> &'static str {
    match source {
        PowerSource::Ac => "da tomada",
        PowerSource::Battery => "da bateria",
    }
}

/// Returns `parent[key]` as a standard table, creating or converting it.
fn table<'a>(parent: &'a mut Table, key: &str) -> &'a mut Table {
    let item = parent.entry(key).or_insert_with(|| {
        let mut t = Table::new();
        t.set_implicit(true);
        Item::Table(t)
    });
    if !item.is_table() {
        let old = std::mem::take(item);
        *item = Item::Table(old.into_table().unwrap_or_default());
    }
    item.as_table_mut().expect("converted to a table above")
}

/// Sets `t[key]`, keeping the key's position and its trailing comment.
fn set_value(t: &mut Table, key: &str, mut value: Value) {
    if let Some(old) = t.get_mut(key).and_then(Item::as_value_mut) {
        *value.decor_mut() = old.decor().clone();
        *old = value;
    } else {
        t.insert(key, Item::Value(value));
    }
}

fn fan_pair_value(pair: FanPair<Boost>) -> InlineTable {
    let mut t = InlineTable::new();
    for (fan, boost) in pair.iter() {
        t.insert(fan.as_str(), i64::from(boost.0).into());
    }
    t.fmt();
    t
}

fn points_value(curve: &Curve, fan: FanId) -> Array {
    curve
        .points(fan)
        .iter()
        .map(|p| {
            let mut point = Array::new();
            point.push(temp_value(p.temp_c));
            point.push(i64::from(p.boost.0));
            Value::Array(point)
        })
        .collect()
}

/// Whole degrees are written as integers, like the shipped config.
#[allow(clippy::cast_possible_truncation, clippy::float_cmp)] // checked integral and in range
fn temp_value(temp_c: f64) -> Value {
    if temp_c.fract() == 0.0 && temp_c.abs() <= curve::MAX_TEMP_C * 10.0 {
        (temp_c as i64).into()
    } else {
        temp_c.into()
    }
}

/// Writes `contents` to `path` via `<path>.tmp`, `fsync` and `rename`,
/// copying the old file to `<path>.bak` first (SPEC 7.1).
pub fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    let with_suffix = |suffix: &str| -> PathBuf {
        let mut s = path.as_os_str().to_owned();
        s.push(suffix);
        s.into()
    };
    let (tmp, bak) = (with_suffix(".tmp"), with_suffix(".bak"));

    if path.exists() {
        fs::copy(path, &bak).map_err(|e| Error::from_io(&bak, e))?;
    }
    let write_tmp = || -> io::Result<()> {
        let mut f = File::create(&tmp)?;
        f.write_all(contents.as_bytes())?;
        // Explicit chmod: the umask would drop the group write bit.
        f.set_permissions(Permissions::from_mode(CONFIG_MODE))?;
        f.sync_all()
    };
    write_tmp().map_err(|e| Error::from_io(&tmp, e))?;
    fs::rename(&tmp, path).map_err(|e| Error::from_io(path, e))?;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        File::open(dir)
            .and_then(|d| d.sync_all())
            .map_err(|e| Error::from_io(dir, e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_config_matches_builtin_defaults() {
        let c = Config::default();
        assert_eq!(c.version, 1);
        assert_eq!(c.hardware, HardwareConfig::default());
        assert_eq!(c.daemon, DaemonConfig::default());
        assert_eq!(c.sensors, default_sensors());
        assert_eq!(c.defaults, Defaults::default());
        assert_eq!(
            c.curves.keys().collect::<Vec<_>>(),
            ["agressivo", "equilibrado", "silencioso"]
        );
    }

    #[test]
    fn missing_sections_use_defaults() {
        let c = Config::parse("version = 1\n").unwrap();
        assert_eq!(c.hardware, HardwareConfig::default());
        assert_eq!(c.defaults, Defaults::default());
        assert!(c.curves.is_empty());
    }

    #[test]
    fn sensor_refs() {
        let s: SensorRef = "alienware_wmi:CPU".parse().unwrap();
        assert_eq!(s, SensorRef::label("alienware_wmi", "CPU"));
        let s: SensorRef = "coretemp:Package id 0".parse().unwrap();
        assert_eq!(s.selector, SensorSelector::Label("Package id 0".into()));
        let s: SensorRef = "dell_ddv#9".parse().unwrap();
        assert_eq!(s.selector, SensorSelector::Index(9));
        assert_eq!(s.to_string(), "dell_ddv#9");
        for bad in [
            "",
            "coretemp",
            ":CPU",
            "coretemp:",
            "dell_ddv#x",
            "dell_ddv#",
        ] {
            assert!(bad.parse::<SensorRef>().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn preset_assign_keeps_unused_fields() {
        let mut p = Defaults::default().ac;
        p.fixed = FanPair::both(Boost(50));
        p.assign(&Preset {
            profile: Profile::Quiet,
            control: Control::Curve("agressivo".into()),
        });
        assert_eq!(p.control, ControlKind::Curve);
        assert_eq!(p.curve, "agressivo");
        assert_eq!(p.fixed, FanPair::both(Boost(50)));
        assert_eq!(p.preset().control, Control::Curve("agressivo".into()));
    }
}
