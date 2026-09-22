//! Runs the real `alienfand` on a private session bus against a copy of the
//! fake sysfs tree, and drives it through the client proxy (SPEC 16.1).

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use alienfan_proto::{AlienFan1Proxy, INTERFACE_XML, error_name};
use futures_util::StreamExt;
use tempfile::TempDir;

const PROFILE: &str = "class/platform-profile/platform-profile-1/profile";
const BOOST1: &str = "class/hwmon/hwmon3/fan1_boost";
const BOOST2: &str = "class/hwmon/hwmon3/fan2_boost";
const AC_ONLINE: &str = "class/power_supply/AC/online";

struct Bus {
    process: Child,
    address: String,
}

impl Bus {
    fn start() -> Option<Self> {
        let mut process = Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address"])
            .stdout(Stdio::piped())
            .spawn()
            .ok()?;
        let mut address = String::new();
        BufReader::new(process.stdout.take()?)
            .read_line(&mut address)
            .ok()?;
        Some(Self {
            process,
            address: address.trim().to_owned(),
        })
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

struct Daemon {
    dir: TempDir,
    process: Child,
}

impl Daemon {
    fn start(bus: &Bus) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/fake-sysfs");
        copy_dir(&fixture, &dir.path().join("sys"));
        fs::write(dir.path().join("config.toml"), "[daemon]\ntick_ms = 250\n").unwrap();
        let process = spawn(bus, dir.path());
        Self { dir, process }
    }

    fn sys(&self, rel: &str) -> PathBuf {
        self.dir.path().join("sys").join(rel)
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.sys(rel)).unwrap().trim().to_owned()
    }

    fn terminate(&mut self) {
        Command::new("kill")
            .args(["-TERM", &self.process.id().to_string()])
            .status()
            .unwrap();
        let status = self.process.wait().unwrap();
        assert!(status.success(), "{status}");
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

fn spawn(bus: &Bus, dir: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_alienfand"))
        .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
        .env("ALIENFAN_SYSFS_ROOT", dir.join("sys"))
        .env("ALIENFAN_CONFIG", dir.join("config.toml"))
        .env_remove("JOURNAL_STREAM")
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
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

async fn eventually<F: AsyncFnMut() -> bool>(what: &str, mut check: F) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !check().await {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "current_thread")]
#[allow(clippy::too_many_lines)] // one scenario on one daemon: starting it is slow
async fn daemon_over_dbus() {
    let Some(bus) = Bus::start() else {
        eprintln!("dbus-daemon not available; skipping");
        return;
    };
    let mut daemon = Daemon::start(&bus);
    let conn = zbus::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .build()
        .await
        .unwrap();
    let proxy = AlienFan1Proxy::builder(&conn)
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
        .unwrap();
    eventually("the daemon on the bus", async || {
        proxy.health().await.is_ok()
    })
    .await;

    // Startup applied the AC default.
    assert_eq!(proxy.health().await.unwrap(), "ok");
    assert_eq!(proxy.power_source().await.unwrap(), "ac");
    assert_eq!(proxy.profile().await.unwrap(), "balanced-performance");
    assert_eq!(proxy.override_until().await.unwrap(), "power-change");
    assert!((proxy.emergency_temp_c().await.unwrap() - 95.0).abs() < f64::EPSILON);

    // Telemetry arrives every tick.
    let mut telemetry = proxy.receive_telemetry().await.unwrap();
    let signal = tokio::time::timeout(Duration::from_secs(2), telemetry.next())
        .await
        .expect("a Telemetry signal")
        .unwrap();
    let args = signal.args().unwrap();
    assert_eq!(args.fans().len(), 2);
    assert_eq!(args.fans()[0].id, "cpu");
    assert!(args.temps().contains_key("alienware_wmi:CPU"));

    // Writes, with PropertiesChanged.
    let cached = AlienFan1Proxy::new(&conn).await.unwrap();
    let mut control_changes = cached.receive_control_changed().await;
    proxy.set_fixed_boost("all", 128).await.unwrap();
    assert_eq!(
        (daemon.read(BOOST1), daemon.read(BOOST2)),
        ("128".into(), "128".into())
    );
    assert!(proxy.override_active().await.unwrap());
    eventually("Control changed to fixed", async || {
        let next = tokio::time::timeout(Duration::from_millis(100), control_changes.next()).await;
        match next {
            Ok(Some(change)) => change.get().await.unwrap() == "fixed",
            _ => false,
        }
    })
    .await;

    proxy.set_profile("quiet").await.unwrap();
    assert_eq!(daemon.read(PROFILE), "quiet");
    assert_eq!(
        daemon.read(BOOST1),
        "128",
        "boost survives the profile change"
    );

    // Errors carry the SPEC 9 names.
    let err = proxy.set_profile("turbo").await.unwrap_err();
    assert_eq!(error_name(&err), Some("InvalidArgument"), "{err}");
    let err = proxy.delete_curve("nope").await.unwrap_err();
    assert_eq!(error_name(&err), Some("InvalidArgument"), "{err}");

    // Defaults and curves round-trip through the config.
    proxy.save_as_default("ac").await.unwrap();
    let (ac, _) = proxy.get_defaults().await.unwrap();
    assert_eq!(ac.profile.as_deref(), Some("quiet"));
    assert_eq!((ac.fixed_cpu, ac.fixed_gpu), (Some(128), Some(128)));
    assert!(!proxy.override_active().await.unwrap());
    let pts = [(40.0, 0), (70.0, 100), (90.0, 255)];
    proxy.save_curve("teste", &pts, &pts).await.unwrap();
    assert!(proxy.list_curves().await.unwrap().contains(&"teste".into()));
    assert_eq!(proxy.get_curve("teste", "gpu").await.unwrap(), pts);

    // Unplugging applies the battery default (polling fallback).
    fs::write(daemon.sys(AC_ONLINE), "0").unwrap();
    eventually("the battery default", async || {
        proxy.power_source().await.unwrap() == "battery"
    })
    .await;
    assert_eq!(daemon.read(PROFILE), "balanced");
    assert_eq!(daemon.read(BOOST1), "0");

    // The introspection matches the published XML.
    let introspect = zbus::fdo::IntrospectableProxy::builder(&conn)
        .destination(alienfan_proto::BUS_NAME)
        .unwrap()
        .path(alienfan_proto::OBJECT_PATH)
        .unwrap()
        .build()
        .await
        .unwrap();
    let live = introspect.introspect().await.unwrap();
    for line in INTERFACE_XML.lines() {
        if let Some(rest) = line.trim().strip_prefix('<')
            && let Some(name) = rest
                .split("name=\"")
                .nth(1)
                .and_then(|n| n.split('"').next())
            && ["method", "property", "signal"]
                .iter()
                .any(|k| rest.starts_with(k))
        {
            assert!(
                live.contains(&format!("name=\"{name}\"")),
                "{name} missing live"
            );
        }
    }

    // A second daemon refuses to start.
    let status = spawn(&bus, daemon.dir.path()).wait().unwrap();
    assert!(!status.success());

    // SIGTERM with a manual boost hands the fans back to the firmware.
    proxy.set_fixed_boost("cpu", 90).await.unwrap();
    daemon.terminate();
    assert_eq!(
        (daemon.read(BOOST1), daemon.read(BOOST2)),
        ("0".into(), "0".into())
    );
}
