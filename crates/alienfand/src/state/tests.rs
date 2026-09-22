//! State logic on a copy of the fake sysfs tree, with a fake clock.

#![allow(clippy::float_cmp)] // fixture temperatures are exact

use std::fs::{self, Permissions};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use alienfan_core::{ConfigFile, ControlKind, PowerSource, PresetConfig, Profile};
use alienfan_proto::{CurveOptions, Error, PresetDict};
use tempfile::TempDir;
use zvariant::Value;

use super::State;
use alienfan_core::SysfsRoot;

const PROFILE: &str = "class/platform-profile/platform-profile-1/profile";
const BOOST1: &str = "class/hwmon/hwmon3/fan1_boost";
const BOOST2: &str = "class/hwmon/hwmon3/fan2_boost";
const TEMP1: &str = "class/hwmon/hwmon3/temp1_input";
const TEMP2: &str = "class/hwmon/hwmon3/temp2_input";
const AC_ONLINE: &str = "class/power_supply/AC/online";

struct Fx {
    dir: TempDir,
    t0: Instant,
}

impl Fx {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/fake-sysfs");
        copy_dir(&fixture, &dir.path().join("sys"));
        Self {
            dir,
            t0: Instant::now(),
        }
    }

    fn with_config(self, text: &str) -> Self {
        fs::write(self.config(), text).unwrap();
        self
    }

    fn sys(&self, rel: &str) -> PathBuf {
        self.dir.path().join("sys").join(rel)
    }

    fn config(&self) -> PathBuf {
        self.dir.path().join("config.toml")
    }

    fn at(&self, secs: u64) -> Instant {
        self.t0 + Duration::from_secs(secs)
    }

    fn state(&self) -> State {
        State::new(
            SysfsRoot::new(self.dir.path().join("sys")),
            self.config(),
            self.t0,
        )
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.sys(rel)).unwrap().trim().to_owned()
    }

    fn write(&self, rel: &str, value: &str) {
        fs::write(self.sys(rel), value).unwrap();
    }

    fn boosts(&self) -> (String, String) {
        (self.read(BOOST1), self.read(BOOST2))
    }
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), dest).unwrap();
        }
    }
}

fn pair(a: &str, b: &str) -> (String, String) {
    (a.into(), b.into())
}

#[allow(clippy::needless_pass_by_value)] // reads better at the call sites
fn is_invalid<T: std::fmt::Debug>(r: Result<T, Error>) -> bool {
    matches!(r, Err(Error::InvalidArgument(_)))
}

/// Root ignores file modes, so permission tests cannot run as root.
fn read_only(path: &Path) -> bool {
    fs::set_permissions(path, Permissions::from_mode(0o444)).unwrap();
    fs::OpenOptions::new().write(true).open(path).is_err()
}

#[test]
fn startup_applies_the_ac_default() {
    let fx = Fx::new();
    let s = fx.state();
    assert_eq!(fx.read(PROFILE), "balanced-performance");
    assert_eq!(fx.boosts(), pair("0", "0"));
    let p = s.props();
    assert_eq!((p.health.as_str(), p.power_source.as_str()), ("ok", "ac"));
    assert_eq!((p.control.as_str(), p.override_active), ("firmware", false));
    assert_eq!(p.available_profiles.len(), 6);
}

#[test]
fn fixed_boost_creates_an_override() {
    let fx = Fx::new();
    let mut s = fx.state();
    s.set_fixed_boost("cpu", 100, fx.at(0)).unwrap();
    assert_eq!(fx.boosts(), pair("100", "0"));
    s.set_fixed_boost("all", 50, fx.at(1)).unwrap();
    assert_eq!(fx.boosts(), pair("50", "50"));
    let p = s.props();
    assert_eq!((p.control.as_str(), p.override_active), ("fixed", true));
    // The profile stays the default's.
    assert_eq!(fx.read(PROFILE), "balanced-performance");
}

#[test]
fn power_change_applies_the_battery_default_and_ends_the_override() {
    let fx = Fx::new();
    let mut s = fx.state();
    s.set_fixed_boost("all", 80, fx.at(0)).unwrap();
    fx.write(AC_ONLINE, "0");
    s.tick(fx.at(1));
    assert_eq!(fx.read(PROFILE), "balanced");
    assert_eq!(fx.boosts(), pair("0", "0"));
    let p = s.props();
    assert_eq!(
        (p.power_source.as_str(), p.override_active),
        ("battery", false)
    );
}

#[test]
fn manual_override_survives_a_power_change() {
    let fx = Fx::new().with_config("[daemon]\noverride_until = \"manual\"\n");
    let mut s = fx.state();
    s.set_profile("quiet", fx.at(0)).unwrap();
    fx.write(AC_ONLINE, "0");
    s.tick(fx.at(1));
    assert_eq!(fx.read(PROFILE), "quiet");
    assert!(s.props().override_active);
    s.restore_default(fx.at(2)).unwrap();
    assert_eq!(fx.read(PROFILE), "balanced");
}

#[test]
fn curve_ramps_toward_the_target() {
    let fx = Fx::new();
    fx.write(TEMP1, "70000");
    fx.write(TEMP2, "70000");
    let mut s = fx.state();
    s.set_control("curve", "equilibrado", fx.at(0)).unwrap();
    // equilibrado at 70 °C is 115; ramps up 40/s.
    let mut seen = Vec::new();
    for t in 1..=4 {
        s.tick(fx.at(t));
        seen.push(fx.read(BOOST1));
    }
    assert_eq!(seen, ["40", "80", "115", "115"]);
    let (fans, temps) = s.telemetry();
    assert_eq!(fans[0].target_boost, 115);
    assert_eq!(fans[0].temp_c, Some(70.0));
    assert_eq!(temps["alienware_wmi:CPU"], 70.0);
    assert_eq!(s.props().active_curve, "equilibrado");
}

#[test]
fn curve_without_a_name_uses_the_default_curve() {
    let fx = Fx::new();
    let mut s = fx.state();
    s.set_control("curve", "", fx.at(0)).unwrap();
    assert_eq!(s.props().active_curve, "equilibrado");
}

#[test]
fn fixed_control_takes_over_the_current_output() {
    let fx = Fx::new();
    fx.write(TEMP1, "70000");
    fx.write(TEMP2, "70000");
    let mut s = fx.state();
    s.set_control("curve", "equilibrado", fx.at(0)).unwrap();
    s.tick(fx.at(1));
    s.tick(fx.at(2));
    s.set_control("fixed", "", fx.at(3)).unwrap();
    assert_eq!(fx.boosts(), pair("80", "80"));
    assert_eq!(s.props().control, "fixed");
}

#[test]
fn save_as_default_persists_and_clears_the_override() {
    let fx = Fx::new();
    let mut s = fx.state();
    s.set_profile("quiet", fx.at(0)).unwrap();
    s.save_as_default("ac", fx.at(1)).unwrap();
    assert!(!s.props().override_active);
    let text = fs::read_to_string(fx.config()).unwrap();
    assert!(text.contains("profile = \"quiet\""), "{text}");

    fx.write(PROFILE, "balanced");
    let _again = fx.state();
    assert_eq!(fx.read(PROFILE), "quiet");
}

#[test]
fn saving_the_other_source_keeps_the_override() {
    let fx = Fx::new();
    let mut s = fx.state();
    s.set_profile("quiet", fx.at(0)).unwrap();
    s.save_as_default("battery", fx.at(1)).unwrap();
    assert!(s.props().override_active);
    assert_eq!(s.get_defaults().1.profile.as_deref(), Some("quiet"));
    assert_eq!(
        s.get_defaults().0.profile.as_deref(),
        Some("balanced-performance")
    );
}

#[test]
fn set_default_applies_only_while_following_defaults() {
    let fx = Fx::new();
    let mut s = fx.state();
    let cool = PresetDict {
        profile: Some("cool".into()),
        ..PresetDict::default()
    };
    s.set_default("ac", &cool, fx.at(0)).unwrap();
    assert_eq!(fx.read(PROFILE), "cool");

    s.set_profile("quiet", fx.at(1)).unwrap();
    let perf = PresetDict {
        profile: Some("balanced".into()),
        ..PresetDict::default()
    };
    s.set_default("ac", &perf, fx.at(2)).unwrap();
    assert_eq!(fx.read(PROFILE), "quiet", "the override wins");
    assert_eq!(s.get_defaults().0.profile.as_deref(), Some("balanced"));
}

#[test]
fn curves_in_use_cannot_be_deleted() {
    let fx = Fx::new();
    let mut s = fx.state();
    s.set_control("curve", "agressivo", fx.at(0)).unwrap();
    assert!(matches!(
        s.delete_curve("agressivo"),
        Err(Error::CurveInUse(_))
    ));

    let curve_default = PresetDict {
        control: Some("curve".into()),
        ..PresetDict::default()
    };
    s.set_default("battery", &curve_default, fx.at(1)).unwrap();
    assert!(matches!(
        s.delete_curve("silencioso"),
        Err(Error::CurveInUse(_))
    ));

    // Named by the AC default, but not its active control.
    s.delete_curve("equilibrado").unwrap();
    assert_eq!(s.list_curves(), ["agressivo", "silencioso"]);
}

#[test]
fn invalid_requests_change_nothing() {
    let fx = Fx::new();
    let mut s = fx.state();
    assert!(is_invalid(s.set_profile("turbo", fx.at(0))));
    assert!(is_invalid(s.set_fixed_boost("fan3", 10, fx.at(0))));
    assert!(is_invalid(s.set_control("curve", "nope", fx.at(0))));
    assert!(is_invalid(s.set_control("auto", "", fx.at(0))));
    assert!(is_invalid(s.save_as_default("desk", fx.at(0))));
    let bad = [(50.0, 100), (60.0, 50)];
    assert!(is_invalid(s.save_curve(
        "x",
        &bad,
        &bad,
        &CurveOptions::default()
    )));
    assert!(is_invalid(s.save_curve(
        "a b",
        &[(40.0, 0), (80.0, 255)],
        &[(40.0, 0), (80.0, 255)],
        &CurveOptions::default()
    )));
    assert!(is_invalid(
        s.set_daemon_option("tick_ms", &Value::from(100u32))
    ));
    assert!(is_invalid(
        s.set_daemon_option("override_until", &Value::from(true))
    ));
    assert!(is_invalid(s.get_curve("equilibrado", "all")));
    assert!(!s.props().override_active);
    assert!(!fx.config().exists(), "nothing was saved");
    assert_eq!(fx.boosts(), pair("0", "0"));
}

#[test]
fn save_curve_keeps_options_and_updates_the_running_curve() {
    let fx = Fx::new();
    fx.write(TEMP1, "60000");
    fx.write(TEMP2, "60000");
    let mut s = fx.state();
    s.set_control("curve", "equilibrado", fx.at(0)).unwrap();
    let pts = [(40.0, 0), (60.0, 20), (80.0, 255)];
    s.save_curve("equilibrado", &pts, &pts, &CurveOptions::default())
        .unwrap();
    let opts = s.get_curve_options("equilibrado").unwrap();
    assert_eq!(opts.ramp_up_per_s, Some(40), "options kept");
    assert_eq!(s.get_curve("equilibrado", "gpu").unwrap(), pts);
    s.tick(fx.at(1));
    assert_eq!(fx.read(BOOST1), "20", "new target at 60 °C");

    let fast = CurveOptions {
        ramp_up_per_s: Some(200),
        ..CurveOptions::default()
    };
    s.save_curve("novo", &pts, &pts, &fast).unwrap();
    assert_eq!(
        s.get_curve_options("novo").unwrap().ramp_up_per_s,
        Some(200)
    );
    assert_eq!(s.get_curve_options("novo").unwrap().hysteresis_c, Some(3.0));
}

#[test]
fn daemon_option_is_saved() {
    let fx = Fx::new();
    let mut s = fx.state();
    s.set_daemon_option("override_until", &Value::from("manual"))
        .unwrap();
    assert_eq!(s.props().override_until, "manual");
    assert!(
        fs::read_to_string(fx.config())
            .unwrap()
            .contains("\"manual\"")
    );
}

#[test]
fn config_edits_on_disk_are_picked_up() {
    let fx = Fx::new();
    let mut s = fx.state();
    let mut file = ConfigFile::default();
    let quiet = PresetConfig {
        profile: Profile::Quiet,
        control: ControlKind::Firmware,
        ..file.config().defaults.get(PowerSource::Ac).clone()
    };
    file.set_default(PowerSource::Ac, &quiet).unwrap();
    file.save(&fx.config()).unwrap();
    s.tick(fx.at(1));
    assert_eq!(fx.read(PROFILE), "quiet");

    fs::write(fx.config(), "version = \n").unwrap();
    s.tick(fx.at(2));
    let p = s.props();
    assert_eq!(p.health, "degraded");
    assert!(p.health_message.contains("linha 1"), "{}", p.health_message);
    assert_eq!(fx.read(PROFILE), "quiet", "keeps the last valid config");

    file.save(&fx.config()).unwrap();
    s.tick(fx.at(3));
    assert_eq!(s.props().health, "ok");
}

#[test]
fn emergency_maxes_both_fans_until_it_cools_down() {
    let fx = Fx::new();
    let mut s = fx.state();
    fx.write(TEMP1, "100000");
    s.tick(fx.at(1));
    assert_eq!(fx.boosts(), pair("255", "255"));
    assert_eq!(s.props().health, "emergency");

    fx.write(TEMP1, "80000");
    for t in 2..=11 {
        s.tick(fx.at(t));
    }
    assert_eq!(s.props().health, "emergency", "calm for 9 s only");
    s.tick(fx.at(12));
    assert_eq!(s.props().health, "ok");
    assert_eq!(fx.boosts(), pair("0", "0"), "back to firmware");
}

#[test]
fn shutdown_hands_manual_fans_back_to_the_firmware() {
    let fx = Fx::new();
    let mut s = fx.state();
    s.set_fixed_boost("all", 100, fx.at(0)).unwrap();
    s.shutdown();
    assert_eq!(fx.boosts(), pair("0", "0"));

    let fx = Fx::new();
    let mut s = fx.state();
    fx.write(BOOST1, "7");
    s.shutdown();
    assert_eq!(fx.read(BOOST1), "7", "firmware control writes nothing");
}

#[test]
fn a_missing_driver_is_reported_and_recovered() {
    let fx = Fx::new();
    let mut s = fx.state();
    s.set_fixed_boost("all", 30, fx.at(0)).unwrap();
    let hwmon = fx.sys("class/hwmon/hwmon3");
    let aside = fx.dir.path().join("hwmon3");
    fs::rename(&hwmon, &aside).unwrap();
    s.tick(fx.at(1));
    assert_eq!(s.props().health, "no-driver");
    assert!(matches!(
        s.set_profile("quiet", fx.at(1)),
        Err(Error::HardwareUnavailable(_))
    ));

    fs::rename(&aside, &hwmon).unwrap();
    fx.write(BOOST1, "0");
    s.tick(fx.at(2));
    assert_eq!(s.props().health, "ok");
    assert_eq!(fx.boosts(), pair("30", "30"), "the override is reapplied");
}

#[test]
fn an_outside_profile_change_rewrites_the_boost() {
    let fx = Fx::new();
    let mut s = fx.state();
    s.set_fixed_boost("all", 100, fx.at(0)).unwrap();
    // As after a key press: the profile changes and the firmware resets
    // the boost.
    fx.write(PROFILE, "quiet");
    fx.write(BOOST1, "0");
    fx.write(BOOST2, "0");
    s.tick(fx.at(1));
    assert_eq!(fx.boosts(), pair("100", "100"));
    assert_eq!(fx.read(PROFILE), "quiet", "not fought over");
}

#[test]
fn lost_permission_is_reported_and_recovered() {
    let fx = Fx::new();
    let mut s = fx.state();
    if !read_only(&fx.sys(BOOST1)) {
        return;
    }
    assert!(matches!(
        s.set_fixed_boost("all", 60, fx.at(0)),
        Err(Error::PermissionDenied(_))
    ));
    s.tick(fx.at(1));
    assert_eq!(s.props().health, "no-permission");

    fs::set_permissions(fx.sys(BOOST1), Permissions::from_mode(0o644)).unwrap();
    s.tick(fx.at(2));
    s.tick(fx.at(3));
    assert_eq!(s.props().health, "ok");
    assert_eq!(fx.boosts(), pair("60", "60"));
}
