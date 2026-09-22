//! The D-Bus contract between `alienfand` and its clients (SPEC 9): names,
//! the interface XML, the `a{sv}` payloads, errors and a client proxy.

use std::collections::HashMap;

use zvariant::{DeserializeDict, SerializeDict, Type, Value};

pub const BUS_NAME: &str = "io.github.lorenzopasquali.AlienFan";
pub const OBJECT_PATH: &str = "/io/github/lorenzopasquali/AlienFan";
pub const INTERFACE: &str = "io.github.lorenzopasquali.AlienFan1";

/// Introspection XML. The GNOME extension keeps a copy in `dbus.js`.
pub const INTERFACE_XML: &str = include_str!("../io.github.lorenzopasquali.AlienFan1.xml");

/// Errors a method can return. Messages are pt-BR.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "io.github.lorenzopasquali.AlienFan1.Error")]
pub enum Error {
    #[zbus(error)]
    ZBus(zbus::Error),
    InvalidArgument(String),
    PermissionDenied(String),
    HardwareUnavailable(String),
    ConfigWriteFailed(String),
    CurveInUse(String),
}

/// One entry of `GetTelemetry`/`Telemetry` `fans`.
#[derive(Debug, Clone, PartialEq, SerializeDict, DeserializeDict, Type)]
#[zvariant(signature = "a{sv}")]
pub struct FanTelemetry {
    /// `cpu` or `gpu`.
    pub id: String,
    pub label: String,
    pub rpm: u32,
    pub rpm_max: u32,
    /// Boost written.
    pub boost: u8,
    /// Boost the control wants, before the ramps.
    pub target_boost: u8,
    /// Temperature of the sensor that drives this fan; absent if unreadable.
    pub temp_c: Option<f64>,
    pub sensor: String,
}

/// `"<hwmon>:<label>"` → °C for every known sensor.
pub type Temps = HashMap<String, f64>;

/// A curve as `(temp °C, boost 0–255)` points.
pub type Points = Vec<(f64, u8)>;

/// A default as `a{sv}`. `GetDefaults` fills every key; `SetDefault` changes
/// only the keys present.
#[derive(Debug, Clone, Default, PartialEq, Eq, SerializeDict, DeserializeDict, Type)]
#[zvariant(signature = "a{sv}")]
pub struct PresetDict {
    pub profile: Option<String>,
    pub control: Option<String>,
    pub fixed_cpu: Option<u8>,
    pub fixed_gpu: Option<u8>,
    pub curve: Option<String>,
}

/// Curve options as `a{sv}`; missing keys keep their value.
#[derive(Debug, Clone, Default, PartialEq, SerializeDict, DeserializeDict, Type)]
#[zvariant(signature = "a{sv}")]
pub struct CurveOptions {
    pub hysteresis_c: Option<f64>,
    pub ramp_up_per_s: Option<u16>,
    pub ramp_down_per_s: Option<u16>,
}

#[zbus::proxy(
    interface = "io.github.lorenzopasquali.AlienFan1",
    default_service = "io.github.lorenzopasquali.AlienFan",
    default_path = "/io/github/lorenzopasquali/AlienFan",
    gen_blocking = false
)]
pub trait AlienFan1 {
    fn get_telemetry(&self) -> zbus::Result<(Vec<FanTelemetry>, Temps)>;
    fn get_defaults(&self) -> zbus::Result<(PresetDict, PresetDict)>;
    fn list_curves(&self) -> zbus::Result<Vec<String>>;
    fn get_curve(&self, name: &str, fan: &str) -> zbus::Result<Points>;
    fn get_curve_options(&self, name: &str) -> zbus::Result<CurveOptions>;

    fn set_profile(&self, profile: &str) -> zbus::Result<()>;
    fn set_fixed_boost(&self, fan: &str, boost: u8) -> zbus::Result<()>;
    fn set_control(&self, control: &str, curve: &str) -> zbus::Result<()>;
    fn restore_default(&self) -> zbus::Result<()>;

    fn save_as_default(&self, target: &str) -> zbus::Result<()>;
    fn set_default(&self, target: &str, preset: &PresetDict) -> zbus::Result<()>;
    fn save_curve(&self, name: &str, cpu: &[(f64, u8)], gpu: &[(f64, u8)]) -> zbus::Result<()>;
    fn save_curve_with_options(
        &self,
        name: &str,
        cpu: &[(f64, u8)],
        gpu: &[(f64, u8)],
        options: &CurveOptions,
    ) -> zbus::Result<()>;
    fn delete_curve(&self, name: &str) -> zbus::Result<()>;
    fn reload_config(&self) -> zbus::Result<()>;
    fn set_daemon_option(&self, name: &str, value: &Value<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    fn telemetry(&self, fans: Vec<FanTelemetry>, temps: Temps) -> zbus::Result<()>;

    #[zbus(property)]
    fn version(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn health(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn health_message(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn power_source(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn profile(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn available_profiles(&self) -> zbus::Result<Vec<String>>;
    #[zbus(property)]
    fn control(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn active_curve(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn override_active(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn boost_requires_custom(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn override_until(&self) -> zbus::Result<String>;
    #[zbus(property, name = "EmergencyTempC")]
    fn emergency_temp_c(&self) -> zbus::Result<f64>;
}

/// The short name (e.g. `InvalidArgument`) of an error returned by the
/// daemon, if it is one of ours.
pub fn error_name(e: &zbus::Error) -> Option<&str> {
    match e {
        zbus::Error::MethodError(name, _, _) => name
            .as_str()
            .strip_prefix(INTERFACE)
            .and_then(|rest| rest.strip_prefix(".Error.")),
        _ => None,
    }
}

/// The human message of a daemon error.
pub fn error_message(e: &zbus::Error) -> String {
    match e {
        zbus::Error::MethodError(_, Some(message), _) => message.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_names_the_interface() {
        assert!(INTERFACE_XML.contains(&format!("<interface name=\"{INTERFACE}\">")));
    }

    #[test]
    fn gnome_extension_has_the_same_xml() {
        let js =
            include_str!("../../../gnome-extension/alienfan@lorenzopasquali.github.io/dbus.js");
        assert!(
            js.contains(&format!("`{INTERFACE_XML}`")),
            "regenerate the XML in the extension's dbus.js"
        );
    }

    #[test]
    fn dict_signatures() {
        assert_eq!(FanTelemetry::SIGNATURE, "a{sv}");
        assert_eq!(PresetDict::SIGNATURE, "a{sv}");
        assert_eq!(<Vec<FanTelemetry>>::SIGNATURE, "aa{sv}");
    }
}
