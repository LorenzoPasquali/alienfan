//! sysfs access (SPEC 6.5).
//!
//! Devices are always found by their `name` file: `hwmonN` numbers change
//! between boots. Only the profile and the two `fanN_boost` nodes are ever
//! written.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use tracing::debug;

use crate::config::{SensorRef, SensorSelector};
use crate::error::{Error, Result};
use crate::model::{Boost, FanId, PowerSource, Profile};
use crate::plan::Write;

pub const PROFILE_NAME: &str = "alienware-wmi";
pub const HWMON_NAME: &str = "alienware_wmi";

/// hwmon devices whose temperatures go into telemetry (SPEC 2.2).
pub const TEMP_SOURCES: &[&str] = &[HWMON_NAME, "coretemp", "dell_ddv", "dell_smm"];

/// Mount point of sysfs. Tests point it at a fixture tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SysfsRoot(PathBuf);

impl SysfsRoot {
    pub const ENV: &'static str = "ALIENFAN_SYSFS_ROOT";

    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    /// `$ALIENFAN_SYSFS_ROOT`, or `/sys`.
    pub fn from_env() -> Self {
        Self::new(env::var_os(Self::ENV).unwrap_or_else(|| "/sys".into()))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    fn class(&self, class: &str) -> PathBuf {
        self.0.join("class").join(class)
    }

    /// Devices of `class` in a stable order.
    fn devices(&self, class: &str) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = fs::read_dir(self.class(class))
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        dirs.sort_by_key(|p| natural_key(p));
        dirs
    }

    fn find_named(&self, class: &str, name: &str) -> Option<PathBuf> {
        self.devices(class)
            .into_iter()
            .find(|dir| read_trimmed(&dir.join("name")).is_ok_and(|n| n == name))
    }

    /// `None` when no `Mains` supply exists (then callers assume AC).
    pub fn power_source(&self) -> Result<Option<PowerSource>> {
        let mut found = None;
        for dir in self.devices("power_supply") {
            if read_trimmed(&dir.join("type")).is_ok_and(|t| t == "Mains") {
                let online = read_trimmed(&dir.join("online"))?;
                if online == "1" {
                    return Ok(Some(PowerSource::Ac));
                }
                found = Some(PowerSource::Battery);
            }
        }
        Ok(found)
    }

    /// Finds the `tempN_input` file of a configured sensor.
    pub fn resolve_sensor(&self, sensor: &SensorRef) -> Result<PathBuf> {
        let not_found = || Error::NoDriver(format!("sensor {sensor} não encontrado"));
        for dir in self.devices("hwmon") {
            if read_trimmed(&dir.join("name")).ok().as_deref() != Some(sensor.hwmon.as_str()) {
                continue;
            }
            let path = match &sensor.selector {
                SensorSelector::Index(n) => Some(dir.join(format!("temp{n}_input"))),
                SensorSelector::Label(label) => temp_indexes(&dir)
                    .into_iter()
                    .find(|n| {
                        read_trimmed(&dir.join(format!("temp{n}_label"))).is_ok_and(|l| &l == label)
                    })
                    .map(|n| dir.join(format!("temp{n}_input"))),
            };
            if let Some(path) = path.filter(|p| p.exists()) {
                return Ok(path);
            }
        }
        Err(not_found())
    }

    /// Every temperature of [`TEMP_SOURCES`], keyed `"<hwmon>:<label>"`, or
    /// `"<hwmon>#<N>"` when the label is missing or repeated. Unreadable
    /// sensors are skipped.
    pub fn read_all_temps(&self) -> Vec<(String, f64)> {
        let mut out = Vec::new();
        let hwmons: Vec<(String, PathBuf)> = self
            .devices("hwmon")
            .into_iter()
            .filter_map(|dir| Some((read_trimmed(&dir.join("name")).ok()?, dir)))
            .collect();
        for source in TEMP_SOURCES {
            for (_, dir) in hwmons.iter().filter(|(name, _)| name == source) {
                let sensors: Vec<(u32, Option<String>)> = temp_indexes(dir)
                    .into_iter()
                    .map(|n| (n, read_trimmed(&dir.join(format!("temp{n}_label"))).ok()))
                    .collect();
                for (n, label) in &sensors {
                    let Ok(temp) = read_temp(&dir.join(format!("temp{n}_input"))) else {
                        continue;
                    };
                    let unique = label.as_ref().filter(|l| {
                        sensors
                            .iter()
                            .filter(|(_, other)| other.as_ref() == Some(l))
                            .count()
                            == 1
                    });
                    let key = match unique {
                        Some(label) => format!("{source}:{label}"),
                        None => format!("{source}#{n}"),
                    };
                    out.push((key, temp));
                }
            }
        }
        out
    }
}

/// The alienware-wmi platform profile and hwmon device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hardware {
    profile_dir: PathBuf,
    hwmon_dir: PathBuf,
}

/// One fan as read from sysfs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FanReading {
    pub label: String,
    pub rpm: u32,
    pub rpm_max: u32,
    pub boost: Boost,
}

impl Hardware {
    /// Cheap enough to repeat after every I/O failure: the module may have
    /// been reloaded.
    pub fn discover(root: &SysfsRoot) -> Result<Self> {
        let profile_dir = root
            .find_named("platform-profile", PROFILE_NAME)
            .ok_or_else(|| {
                Error::NoDriver(format!(
                    "perfil térmico {PROFILE_NAME} (o módulo alienware_wmi está carregado?)"
                ))
            })?;
        let hwmon_dir = root.find_named("hwmon", HWMON_NAME).ok_or_else(|| {
            Error::NoDriver(format!(
                "hwmon {HWMON_NAME} (o módulo alienware_wmi está carregado?)"
            ))
        })?;
        Ok(Self {
            profile_dir,
            hwmon_dir,
        })
    }

    pub fn profile_path(&self) -> PathBuf {
        self.profile_dir.join("profile")
    }

    pub fn boost_path(&self, fan: FanId) -> PathBuf {
        self.fan_file(fan, "boost")
    }

    fn fan_file(&self, fan: FanId, attr: &str) -> PathBuf {
        self.hwmon_dir
            .join(format!("fan{}_{attr}", fan.sysfs_index()))
    }

    pub fn profile(&self) -> Result<Profile> {
        read_trimmed(&self.profile_path())?.parse()
    }

    pub fn choices(&self) -> Result<Vec<Profile>> {
        Ok(Profile::parse_choices(&read_trimmed(
            &self.profile_dir.join("choices"),
        )?))
    }

    /// Returns whether it wrote: nothing is written when the value is
    /// already set, because each write is a WMI call.
    pub fn set_profile(&self, profile: Profile) -> Result<bool> {
        let choices = self.choices()?;
        if !choices.contains(&profile) {
            let names: Vec<_> = choices.iter().map(|p| p.as_str()).collect();
            return Err(Error::invalid(format!(
                "perfil \"{profile}\" não está disponível (disponíveis: {})",
                names.join(", ")
            )));
        }
        write_if_changed(&self.profile_path(), profile.as_str(), false)
    }

    pub fn boost(&self, fan: FanId) -> Result<Boost> {
        read_parsed(&self.boost_path(fan)).map(Boost)
    }

    /// Like [`Hardware::set_profile`]; `force` skips the read-back check.
    pub fn set_boost(&self, fan: FanId, boost: Boost, force: bool) -> Result<bool> {
        write_if_changed(&self.boost_path(fan), &boost.to_string(), force)
    }

    pub fn fan(&self, fan: FanId) -> Result<FanReading> {
        Ok(FanReading {
            label: read_trimmed(&self.fan_file(fan, "label"))?,
            rpm: read_parsed(&self.fan_file(fan, "input"))?,
            rpm_max: read_parsed(&self.fan_file(fan, "max"))?,
            boost: self.boost(fan)?,
        })
    }

    /// Temperature of the sensor the EC pairs with `fan`, in °C.
    pub fn temp(&self, fan: FanId) -> Result<f64> {
        read_temp(
            &self
                .hwmon_dir
                .join(format!("temp{}_input", fan.sysfs_index())),
        )
    }

    /// Runs a plan in order. Returns how many writes reached the hardware.
    pub fn apply(&self, writes: &[Write]) -> Result<usize> {
        let mut written = 0;
        for w in writes {
            let changed = match *w {
                Write::Profile(p) => self.set_profile(p)?,
                Write::Boost { fan, boost, force } => self.set_boost(fan, boost, force)?,
            };
            written += usize::from(changed);
        }
        Ok(written)
    }
}

fn read_trimmed(path: &Path) -> Result<String> {
    fs::read_to_string(path)
        .map(|s| s.trim().to_owned())
        .map_err(|e| Error::from_io(path, e))
}

fn read_parsed<T: std::str::FromStr>(path: &Path) -> Result<T> {
    let s = read_trimmed(path)?;
    s.parse().map_err(|_| Error::Io {
        path: path.to_owned(),
        source: std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("valor inesperado: \"{s}\""),
        ),
    })
}

/// Reads m°C and returns °C.
pub fn read_temp(path: &Path) -> Result<f64> {
    read_parsed::<i32>(path).map(|m| f64::from(m) / 1000.0)
}

/// Whether this process may write `path`. Opening a sysfs attribute does
/// not call the driver; only a write does.
pub fn is_writable(path: &Path) -> bool {
    fs::OpenOptions::new().write(true).open(path).is_ok()
}

fn write_if_changed(path: &Path, value: &str, force: bool) -> Result<bool> {
    if !force && read_trimmed(path)? == value {
        return Ok(false);
    }
    debug!(path = %path.display(), value, "sysfs write");
    fs::write(path, value).map_err(|e| Error::from_io(path, e))?;
    Ok(true)
}

/// The `N`s of the `tempN_input` files in an hwmon directory, sorted.
fn temp_indexes(dir: &Path) -> Vec<u32> {
    let mut indexes: Vec<u32> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let name = e.ok()?.file_name().into_string().ok()?;
            name.strip_prefix("temp")?
                .strip_suffix("_input")?
                .parse()
                .ok()
        })
        .collect();
    indexes.sort_unstable();
    indexes
}

/// Sorts `hwmon10` after `hwmon9`.
fn natural_key(path: &Path) -> (String, u64) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let digits = name.len() - name.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let (stem, num) = name.split_at(name.len() - digits);
    (stem.to_owned(), num.parse().unwrap_or(0))
}
