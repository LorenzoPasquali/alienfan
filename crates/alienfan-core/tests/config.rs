use std::fs;
use std::os::unix::fs::PermissionsExt;

use alienfan_core::config::DEFAULT_CONFIG;
use alienfan_core::{
    Boost, Config, ConfigFile, ControlKind, Curve, CurvePoint, Error, FanPair, OverrideUntil,
    PowerSource, PresetConfig, Profile,
};

fn line_of<'a>(text: &'a str, start: &str) -> &'a str {
    text.lines().find(|l| l.starts_with(start)).unwrap()
}

#[test]
fn unchanged_file_round_trips_byte_for_byte() {
    assert_eq!(ConfigFile::default().text(), DEFAULT_CONFIG);
}

#[test]
fn set_default_keeps_comments_and_layout() {
    let mut file = ConfigFile::default();
    let preset = PresetConfig {
        profile: Profile::Quiet,
        control: ControlKind::Fixed,
        fixed: FanPair::new(Boost(40), Boost(60)),
        curve: "equilibrado".into(),
    };
    file.set_default(PowerSource::Ac, &preset).unwrap();
    let text = file.text();

    assert!(text.starts_with("# alienfan — configuração."));
    assert_eq!(
        line_of(&text, "control = \"fixed\""),
        "control = \"fixed\"            # \"firmware\" | \"fixed\" | \"curve\""
    );
    assert_eq!(
        line_of(&text, "fixed = { cpu = 40"),
        "fixed = { cpu = 40, gpu = 60 }    # usado quando control = \"fixed\""
    );
    assert_eq!(file.config().defaults.ac, preset);
    // The battery default is untouched.
    assert_eq!(
        file.config().defaults.battery,
        Config::default().defaults.battery
    );
    // What we write is what we parse.
    assert_eq!(&Config::parse(&text).unwrap(), file.config());
}

#[test]
fn set_curve_creates_and_replaces() {
    let mut file = ConfigFile::default();
    let points = vec![
        CurvePoint::new(30.0, 0),
        CurvePoint::new(62.5, 128),
        CurvePoint::new(85.0, 255),
    ];
    let curve = Curve::new(FanPair::both(points), 4.5, 60, 5).unwrap();

    file.set_curve("novo", &curve).unwrap();
    assert_eq!(file.config().curve("novo").unwrap(), &curve);
    let text = file.text();
    assert!(text.contains("[curves.novo]"), "{text}");
    assert!(
        text.contains("cpu = [[30, 0], [62.5, 128], [85, 255]]"),
        "{text}"
    );

    file.set_curve("silencioso", &curve).unwrap();
    assert_eq!(file.config().curve("silencioso").unwrap(), &curve);
    assert_eq!(&Config::parse(&file.text()).unwrap(), file.config());
    // Replacing keeps the table where it was.
    let text = file.text();
    assert!(text.find("[curves.silencioso]") < text.find("[curves.equilibrado]"));
}

#[test]
fn set_curve_rejects_bad_names() {
    let mut file = ConfigFile::default();
    let curve = Config::default().curves["silencioso"].clone();
    assert!(matches!(
        file.set_curve("a b", &curve),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(file.set_curve("", &curve), Err(Error::Invalid(_))));
}

#[test]
fn remove_curve_refuses_a_curve_in_use() {
    let mut file = ConfigFile::default();
    file.remove_curve("agressivo").unwrap();
    assert!(!file.text().contains("agressivo"));
    assert!(matches!(
        file.remove_curve("agressivo"),
        Err(Error::Invalid(_))
    ));

    let mut ac = file.config().defaults.ac.clone();
    ac.control = ControlKind::Curve;
    file.set_default(PowerSource::Ac, &ac).unwrap();
    let err = file.remove_curve("equilibrado").unwrap_err();
    assert!(matches!(err, Error::CurveInUse { .. }), "{err}");
    assert!(err.to_string().contains("da tomada"), "{err}");
    // Named by an inactive default: free to go.
    file.remove_curve("silencioso").unwrap();
}

#[test]
fn set_override_until() {
    let mut file = ConfigFile::default();
    file.set_override_until(OverrideUntil::Manual).unwrap();
    assert_eq!(file.config().daemon.override_until, OverrideUntil::Manual);
    assert!(
        file.text()
            .contains("override_until = \"manual\" # \"power-change\" | \"manual\"")
    );
}

#[test]
fn syntax_error_has_line_and_column() {
    let err = Config::parse("version = 1\n[hardware]\nemergency_temp_c = = 3\n").unwrap_err();
    assert_eq!(err.line, Some(3), "{err}");
    assert!(err.column.is_some(), "{err}");
    assert!(err.to_string().starts_with("linha 3, coluna "), "{err}");
}

#[test]
fn type_error_has_line_and_column() {
    let err = Config::parse("version = 1\n\n[daemon]\ntick_ms = \"rápido\"\n").unwrap_err();
    assert_eq!((err.line, err.column), (Some(4), Some(11)), "{err}");
}

#[test]
fn unknown_values_are_rejected_with_position() {
    let err =
        Config::parse("[defaults.ac]\nprofile = \"turbo\"\ncontrol = \"firmware\"\n").unwrap_err();
    assert_eq!(err.line, Some(2), "{err}");
    assert!(err.message.contains("turbo"), "{err}");

    let err = Config::parse("[hardware]\nboost_require_custom = true\n").unwrap_err();
    assert_eq!(err.line, Some(2), "{err}");
}

#[test]
fn invalid_curve_points_at_the_curve() {
    let text = "version = 1\n[curves.x]\ncpu = [[50, 0], [40, 10]]\ngpu = [[50, 0], [60, 10]]\n";
    let err = Config::parse(text).unwrap_err();
    assert!(err.message.contains("estritamente crescentes"), "{err}");
    assert!(err.line.is_some(), "{err}");
}

#[test]
fn cross_references_are_checked() {
    let text = "[defaults.ac]\nprofile = \"quiet\"\ncontrol = \"curve\"\ncurve = \"nope\"\n";
    let err = Config::parse(text).unwrap_err();
    assert!(err.message.contains("defaults.ac.curve"), "{err}");

    let err = Config::parse("version = 2\n").unwrap_err();
    assert!(err.message.contains("version"), "{err}");
    assert!(Config::parse("[daemon]\ntick_ms = 100\n").is_err());
    assert!(Config::parse("[hardware]\nemergency_temp_c = 120.0\n").is_err());
}

#[test]
fn save_is_atomic_with_backup_and_group_write() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");

    // Missing file: shipped defaults.
    let mut file = ConfigFile::load(&path).unwrap();
    assert_eq!(file.text(), DEFAULT_CONFIG);

    file.save(&path).unwrap();
    assert!(!dir.path().join("config.toml.bak").exists());
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o664);

    file.set_override_until(OverrideUntil::Manual).unwrap();
    file.save(&path).unwrap();
    assert_eq!(
        fs::read_to_string(dir.path().join("config.toml.bak")).unwrap(),
        DEFAULT_CONFIG
    );
    assert!(!dir.path().join("config.toml.tmp").exists());

    let loaded = ConfigFile::load(&path).unwrap();
    assert_eq!(loaded.config().daemon.override_until, OverrideUntil::Manual);
}

#[test]
fn load_reports_invalid_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "version = \n").unwrap();
    let err = ConfigFile::load(&path).unwrap_err();
    let Error::Config(e) = err else {
        panic!("{err}")
    };
    assert_eq!(e.line, Some(1));
}

#[test]
fn removing_every_curve_keeps_none() {
    let mut file = ConfigFile::default();
    for name in ["agressivo", "equilibrado", "silencioso"] {
        file.remove_curve(name).unwrap();
    }
    assert!(file.config().curves.is_empty());
    assert!(file.text().contains("[curves]"), "{}", file.text());
    assert!(Config::parse(&file.text()).unwrap().curves.is_empty());
}

#[test]
fn editing_curves_of_a_file_without_curves_keeps_the_shipped_ones() {
    let mut file = ConfigFile::parse("[daemon]\ntick_ms = 500\n").unwrap();
    assert_eq!(file.config().curves.len(), 3);
    let curve = Config::default().curves["silencioso"].clone();
    file.set_curve("nova", &curve).unwrap();
    assert_eq!(
        file.config().curves.keys().collect::<Vec<_>>(),
        ["agressivo", "equilibrado", "nova", "silencioso"]
    );
    let mut file = ConfigFile::parse("version = 1\n").unwrap();
    file.remove_curve("agressivo").unwrap();
    assert_eq!(file.config().curves.len(), 2);
}
