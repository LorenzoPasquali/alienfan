mod common;

use std::fs::{self, Permissions};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use alienfan_core::plan::{Target, plan};
use alienfan_core::{
    Boost, Control, Error, FanId, FanPair, Hardware, PowerSource, Preset, Profile, SensorRef,
};
use common::{fake_sysfs, read, snapshot, write};

fn make_read_only(path: &Path) -> bool {
    fs::set_permissions(path, Permissions::from_mode(0o444)).unwrap();
    // Root ignores file modes: those checks cannot run as root.
    fs::OpenOptions::new().write(true).open(path).is_err()
}

#[test]
fn discovery_matches_by_name_not_number() {
    let (_dir, root) = fake_sysfs();
    let hw = Hardware::discover(&root).unwrap();
    assert!(hw.profile_path().ends_with("platform-profile-1/profile"));
    assert!(hw.boost_path(FanId::Cpu).ends_with("hwmon3/fan1_boost"));
    assert!(hw.boost_path(FanId::Gpu).ends_with("hwmon3/fan2_boost"));
    assert_eq!(hw.profile().unwrap(), Profile::Balanced);
    assert_eq!(hw.choices().unwrap().len(), 6);
}

#[test]
fn discovery_without_driver() {
    let (_dir, root) = fake_sysfs();
    fs::remove_dir_all(root.path().join("class/hwmon/hwmon3")).unwrap();
    assert!(matches!(Hardware::discover(&root), Err(Error::NoDriver(_))));

    let (_dir, root) = fake_sysfs();
    fs::remove_dir_all(
        root.path()
            .join("class/platform-profile/platform-profile-1"),
    )
    .unwrap();
    assert!(matches!(Hardware::discover(&root), Err(Error::NoDriver(_))));
}

#[test]
fn reads_fans_and_temps() {
    let (_dir, root) = fake_sysfs();
    let hw = Hardware::discover(&root).unwrap();
    let cpu = hw.fan(FanId::Cpu).unwrap();
    assert_eq!(cpu.label, "CPU Fan");
    assert_eq!((cpu.rpm, cpu.rpm_max, cpu.boost), (1718, 6000, Boost(0)));
    assert_eq!(hw.fan(FanId::Gpu).unwrap().label, "GPU Fan");
    assert!((hw.temp(FanId::Cpu).unwrap() - 46.0).abs() < f64::EPSILON);
    assert!((hw.temp(FanId::Gpu).unwrap() - 38.0).abs() < f64::EPSILON);
}

#[test]
fn profile_write_is_idempotent() {
    let (_dir, root) = fake_sysfs();
    let hw = Hardware::discover(&root).unwrap();
    assert!(hw.set_profile(Profile::Quiet).unwrap());
    assert_eq!(
        read(&root, "class/platform-profile/platform-profile-1/profile"),
        "quiet"
    );
    if make_read_only(&hw.profile_path()) {
        // A second write would fail on the read-only node.
        assert!(!hw.set_profile(Profile::Quiet).unwrap());
    }
}

#[test]
fn profile_must_be_in_choices() {
    let (_dir, root) = fake_sysfs();
    write(
        &root,
        "class/platform-profile/platform-profile-1/choices",
        "quiet balanced",
    );
    let hw = Hardware::discover(&root).unwrap();
    let err = hw.set_profile(Profile::Performance).unwrap_err();
    assert!(matches!(err, Error::Invalid(_)), "{err}");
    assert!(err.to_string().contains("não está disponível"), "{err}");
    assert_eq!(
        read(&root, "class/platform-profile/platform-profile-1/profile"),
        "balanced"
    );
}

#[test]
fn boost_write_is_idempotent_unless_forced() {
    let (_dir, root) = fake_sysfs();
    let hw = Hardware::discover(&root).unwrap();
    assert!(hw.set_boost(FanId::Gpu, Boost(128), false).unwrap());
    assert_eq!(read(&root, "class/hwmon/hwmon3/fan2_boost"), "128");
    assert_eq!(hw.boost(FanId::Gpu).unwrap(), Boost(128));
    if make_read_only(&hw.boost_path(FanId::Gpu)) {
        assert!(!hw.set_boost(FanId::Gpu, Boost(128), false).unwrap());
        let err = hw.set_boost(FanId::Gpu, Boost(128), true).unwrap_err();
        assert!(matches!(err, Error::NoPermission { .. }), "{err}");
    }
}

#[test]
fn permission_denied_is_reported() {
    let (_dir, root) = fake_sysfs();
    let hw = Hardware::discover(&root).unwrap();
    if make_read_only(&hw.profile_path()) {
        let err = hw.set_profile(Profile::Quiet).unwrap_err();
        assert!(matches!(err, Error::NoPermission { .. }), "{err}");
    }
}

#[test]
fn applying_a_plan_writes_profile_then_boosts_and_nothing_else() {
    let (_dir, root) = fake_sysfs();
    let hw = Hardware::discover(&root).unwrap();
    let before = snapshot(&root);

    let preset = Preset {
        profile: Profile::Quiet,
        control: Control::Fixed(FanPair::new(Boost(10), Boost(20))),
    };
    let target = Target::resolve(&preset, false, FanPair::default());
    let written = hw.apply(&plan(&target, hw.profile().unwrap())).unwrap();
    assert_eq!(written, 3);

    let changed: Vec<_> = snapshot(&root)
        .into_iter()
        .zip(before)
        .filter(|(after, before)| after != before)
        .map(|(after, _)| after.0.strip_prefix(root.path()).unwrap().to_owned())
        .collect();
    assert_eq!(
        changed,
        [
            Path::new("class/hwmon/hwmon3/fan1_boost"),
            Path::new("class/hwmon/hwmon3/fan2_boost"),
            Path::new("class/platform-profile/platform-profile-1/profile"),
        ]
    );
    assert_eq!(read(&root, "class/hwmon/hwmon3/fan1_boost"), "10");
    assert_eq!(read(&root, "class/hwmon/hwmon3/fan2_boost"), "20");
}

#[test]
fn boost_requires_custom_switches_to_custom() {
    let (_dir, root) = fake_sysfs();
    let hw = Hardware::discover(&root).unwrap();
    let preset = Preset {
        profile: Profile::Quiet,
        control: Control::Fixed(FanPair::both(Boost(50))),
    };
    let target = Target::resolve(&preset, true, FanPair::default());
    hw.apply(&plan(&target, hw.profile().unwrap())).unwrap();
    assert_eq!(hw.profile().unwrap(), Profile::Custom);
    assert_eq!(hw.boost(FanId::Cpu).unwrap(), Boost(50));
}

#[test]
fn power_source_follows_mains() {
    let (_dir, root) = fake_sysfs();
    assert_eq!(root.power_source().unwrap(), Some(PowerSource::Ac));
    write(&root, "class/power_supply/AC/online", "0");
    assert_eq!(root.power_source().unwrap(), Some(PowerSource::Battery));
    fs::remove_dir_all(root.path().join("class/power_supply/AC")).unwrap();
    assert_eq!(root.power_source().unwrap(), None);
}

#[test]
fn sensors_resolve_by_label_or_index() {
    let (_dir, root) = fake_sysfs();
    let resolve = |s: &str| root.resolve_sensor(&s.parse::<SensorRef>().unwrap());
    assert!(
        resolve("alienware_wmi:CPU")
            .unwrap()
            .ends_with("hwmon3/temp1_input")
    );
    assert!(
        resolve("alienware_wmi:GPU")
            .unwrap()
            .ends_with("hwmon3/temp2_input")
    );
    assert!(
        resolve("coretemp:Package id 0")
            .unwrap()
            .ends_with("hwmon1/temp1_input")
    );
    assert!(
        resolve("dell_ddv#9")
            .unwrap()
            .ends_with("hwmon4/temp9_input")
    );
    assert!(matches!(resolve("dell_ddv#10"), Err(Error::NoDriver(_))));
    assert!(matches!(
        resolve("alienware_wmi:SSD"),
        Err(Error::NoDriver(_))
    ));
    assert!(matches!(resolve("nvme:Composite"), Err(Error::NoDriver(_))));
}

#[test]
fn all_temps_use_index_keys_for_repeated_labels() {
    let (_dir, root) = fake_sysfs();
    let temps = root.read_all_temps();
    let keys: Vec<&str> = temps.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        [
            "alienware_wmi:CPU",
            "alienware_wmi:GPU",
            "coretemp:Package id 0",
            "coretemp:Core 0",
            "dell_ddv#1",
            "dell_ddv#2",
            "dell_ddv#3",
            "dell_ddv#4",
            "dell_ddv#5",
            "dell_ddv#6",
            "dell_ddv:Memory",
            "dell_ddv:Unknown",
            "dell_ddv:Video",
            "dell_smm#1",
        ]
    );
    assert!((temps[0].1 - 46.0).abs() < f64::EPSILON);
}
