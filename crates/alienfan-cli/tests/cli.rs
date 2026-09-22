//! Runs the `alienfan` binary against a copy of the fake sysfs tree.

use std::fs::{self, Permissions};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

struct Env {
    dir: TempDir,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/fake-sysfs");
        copy_dir(&fixture, &dir.path().join("sys"));
        Self { dir }
    }

    fn sys(&self, rel: &str) -> PathBuf {
        self.dir.path().join("sys").join(rel)
    }

    fn config(&self) -> PathBuf {
        self.dir.path().join("config.toml")
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.sys(rel)).unwrap().trim().to_owned()
    }

    fn write(&self, rel: &str, value: &str) {
        fs::write(self.sys(rel), value).unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_alienfan"))
            .args(args)
            .env("ALIENFAN_SYSFS_ROOT", self.dir.path().join("sys"))
            .env("ALIENFAN_CONFIG", self.config())
            .output()
            .unwrap()
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

fn code(out: &Output) -> i32 {
    out.status.code().unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

const PROFILE: &str = "class/platform-profile/platform-profile-1/profile";
const BOOST1: &str = "class/hwmon/hwmon3/fan1_boost";
const BOOST2: &str = "class/hwmon/hwmon3/fan2_boost";

#[test]
fn status_json_matches_snapshot() {
    let env = Env::new();
    let out = env.run(&["status", "--json"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let snapshot = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/status.json");
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        fs::write(&snapshot, stdout(&out)).unwrap();
    }
    assert_eq!(stdout(&out), fs::read_to_string(snapshot).unwrap());
}

#[test]
fn boost_writes_both_fans_and_warns() {
    let env = Env::new();
    let out = env.run(&["boost", "all", "60%"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        (env.read(BOOST1), env.read(BOOST2)),
        ("153".into(), "153".into())
    );
    assert!(stderr(&out).contains("daemon parado"), "{}", stderr(&out));

    let out = env.run(&["boost", "gpu", "0"]);
    assert_eq!(code(&out), 0);
    assert_eq!(
        (env.read(BOOST1), env.read(BOOST2)),
        ("153".into(), "0".into())
    );

    let status = stdout(&env.run(&["status", "--json"]));
    assert!(status.contains("\"control\": \"fixed\""), "{status}");
}

#[test]
fn boost_requires_custom_moves_to_custom() {
    let env = Env::new();
    fs::write(env.config(), "[hardware]\nboost_requires_custom = true\n").unwrap();
    let out = env.run(&["boost", "cpu", "10"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(env.read(PROFILE), "custom");
    assert_eq!(env.read(BOOST1), "10");
}

#[test]
fn profile_set_keeps_the_boost() {
    let env = Env::new();
    env.write(BOOST1, "77");
    let out = env.run(&["profile", "set", "quiet"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(env.read(PROFILE), "quiet");
    assert_eq!(env.read(BOOST1), "77");
}

#[test]
fn invalid_arguments_exit_2() {
    let env = Env::new();
    for args in [
        &["boost", "cpu", "300"][..],
        &["boost", "cpu", "101%"],
        &["boost", "fan3", "10"],
        &["profile", "set", "turbo"],
        &["curve", "set", "x", "--cpu", "50:10,40:20"],
        &["curve", "show", "nope"],
    ] {
        let out = env.run(args);
        assert_eq!(code(&out), 2, "{args:?}: {}", stderr(&out));
    }
    assert_eq!(env.read(BOOST1), "0");
}

#[test]
fn unavailable_profile_exits_2() {
    let env = Env::new();
    env.write(
        "class/platform-profile/platform-profile-1/choices",
        "quiet balanced",
    );
    let out = env.run(&["profile", "set", "performance"]);
    assert_eq!(code(&out), 2);
    assert!(
        stderr(&out).contains("não está disponível"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn no_permission_exits_3() {
    let env = Env::new();
    let path = env.sys(PROFILE);
    fs::set_permissions(&path, Permissions::from_mode(0o444)).unwrap();
    if fs::OpenOptions::new().write(true).open(&path).is_ok() {
        return; // root ignores file modes
    }
    let out = env.run(&["profile", "set", "quiet"]);
    assert_eq!(code(&out), 3, "{}", stderr(&out));
    let status = stdout(&env.run(&["status", "--json"]));
    assert!(status.contains("\"health\": \"no-permission\""), "{status}");
}

#[test]
fn missing_driver_exits_4_except_at_boot() {
    let env = Env::new();
    fs::remove_dir_all(env.sys("class/hwmon/hwmon3")).unwrap();
    assert_eq!(code(&env.run(&["boost", "cpu", "10"])), 4);
    assert_eq!(code(&env.run(&["status"])), 4);
    assert_eq!(code(&env.run(&["apply"])), 4);
    let out = env.run(&["apply", "--boot", "--wait", "0"]);
    assert_eq!(code(&out), 0);
    assert!(stderr(&out).contains("nada aplicado"), "{}", stderr(&out));
}

#[test]
fn curve_control_needs_the_daemon() {
    let env = Env::new();
    let out = env.run(&["control", "curve", "--curve", "equilibrado"]);
    assert_eq!(code(&out), 5, "{}", stderr(&out));
    assert_eq!(code(&env.run(&["control", "curve", "--curve", "nope"])), 2);
}

#[test]
fn control_firmware_zeroes_the_boost() {
    let env = Env::new();
    env.write(BOOST1, "100");
    env.write(BOOST2, "50");
    assert_eq!(code(&env.run(&["control", "firmware"])), 0);
    assert_eq!(
        (env.read(BOOST1), env.read(BOOST2)),
        ("0".into(), "0".into())
    );
}

#[test]
fn apply_uses_the_default_of_the_power_source() {
    let env = Env::new();
    env.write(BOOST1, "100");
    let out = env.run(&["apply"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(env.read(PROFILE), "balanced-performance");
    // The profile change gives the boost back to the firmware.
    assert_eq!(env.read(BOOST1), "100");
    // With the profile already right, a leftover boost is cleared.
    assert_eq!(code(&env.run(&["apply"])), 0);
    assert_eq!(env.read(BOOST1), "0");

    env.write("class/power_supply/AC/online", "0");
    assert_eq!(code(&env.run(&["apply", "--boot"])), 0);
    assert_eq!(env.read(PROFILE), "balanced");
}

#[test]
fn apply_boot_never_fails_on_a_broken_config() {
    let env = Env::new();
    fs::write(env.config(), "version = \n").unwrap();
    assert_eq!(code(&env.run(&["apply"])), 1);
    let out = env.run(&["apply", "--boot"]);
    assert_eq!(code(&out), 0);
    assert!(stderr(&out).contains("linha 1"), "{}", stderr(&out));
    assert_eq!(env.read(PROFILE), "balanced");

    let status = stdout(&env.run(&["status", "--json"]));
    assert!(status.contains("\"health\": \"degraded\""), "{status}");
    // Manual control still works with the shipped defaults.
    assert_eq!(code(&env.run(&["boost", "cpu", "5"])), 0);
}

#[test]
fn curve_set_and_default_save_write_the_config() {
    let env = Env::new();
    let out = env.run(&[
        "curve",
        "set",
        "novo",
        "--cpu",
        "40:0,80:100%",
        "--ramp-up",
        "60",
    ]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let show = stdout(&env.run(&["curve", "show", "novo"]));
    assert!(show.contains("CPU: 40 °C → 0%, 80 °C → 100%"), "{show}");
    assert!(show.contains("GPU: 40 °C → 0%, 80 °C → 100%"), "{show}");
    assert!(show.contains("subida 60/s"), "{show}");

    env.write(BOOST1, "51");
    assert_eq!(code(&env.run(&["default", "save", "--battery"])), 0);
    let text = fs::read_to_string(env.config()).unwrap();
    assert!(text.contains("[curves.novo]"), "{text}");
    let shown = stdout(&env.run(&["default", "show"]));
    assert!(
        shown.contains("Na bateria:  Equilibrado (balanced) · Manual (CPU 20% · GPU 0%)"),
        "{shown}"
    );
    assert!(shown.contains("Na tomada:   Equilibrado+"), "{shown}");
}
