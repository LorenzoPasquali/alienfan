# alienfan

Thermal profile and fan control for the Alienware 16 Aurora (AC16250) on
Zorin OS 18 / GNOME 46. It replaces the "boot Windows, tweak AWCC, come back"
routine. The full design is in [SPEC.md](SPEC.md).

## Status

| Milestone | What | State |
|---|---|---|
| M0 | Hardware validation (`docs/fase0.sh`, `docs/HARDWARE.md`) | waiting for the run |
| M1 | `alienfan-core`: model, config, curve engine, sysfs | done |
| M2 | `alienfan` CLI (direct mode), udev rule, boot service, TLP drop-in, installer | done |
| M3 | `alienfand` daemon over D-Bus (curve, power source, resume) | next |
| M4 | GNOME Quick Settings extension | |
| M5 | Tauri panel | |
| M6 | awcc removal, final uninstaller, docs | |

## Build and test

Needs Rust (rustup, stable). Nothing here needs root.

```bash
cargo build                 # target/debug/alienfan
cargo test                  # runs against a fake sysfs tree, never the hardware
cargo clippy --all-targets -- -D warnings
```

Read-only commands work on the real machine without installing anything:

```bash
./target/debug/alienfan status
./target/debug/alienfan profile list
./target/debug/alienfan doctor
```

To try write commands without touching the hardware, point the CLI at a copy
of the fixture:

```bash
cp -r tests/fixtures/fake-sysfs /tmp/fake-sys
ALIENFAN_SYSFS_ROOT=/tmp/fake-sys ALIENFAN_CONFIG=/tmp/fake-config.toml \
  ./target/debug/alienfan boost all 60%
```

## Install

```bash
./packaging/install.sh      # run as your user; it asks for sudo and prints each sudo command
```

It builds in release mode, creates the `alienfan` group, installs
`/usr/local/bin/alienfan`, the udev rule, `/etc/alienfan/config.toml` (only if
missing), the TLP drop-in and `alienfan-apply.service`. Log out and back in
so the group applies, then everything works without sudo:

```bash
alienfan doctor
alienfan profile set quiet
alienfan boost all 60%
alienfan default save --ac
```

`./packaging/uninstall.sh` undoes it.

Until M3 nothing switches the profile when you plug or unplug the charger:
the TLP drop-in stops TLP from doing it, and the daemon does not exist yet.
Run `alienfan apply` by hand after a switch.

## CLI

```
alienfan status [--json] [--watch]
alienfan profile list | set <profile>
alienfan boost <cpu|gpu|all> <0-255|0-100%>
alienfan control <firmware|fixed|curve> [--curve <name>]
alienfan curve list | show <name> | set <name> --cpu "45:0,70:45%,90:100%" [--gpu …]
alienfan default show | save [--ac|--battery|--both] | reset
alienfan apply [--boot] [--wait <s>]
alienfan doctor [--json]
```

Exit codes: 0 ok, 1 generic, 2 invalid argument, 3 no permission,
4 hardware not found, 5 daemon error.

Until the daemon exists, every command writes sysfs directly and warns that
nothing keeps the setting across power changes. `control curve` needs the
daemon.

## Decisions not fixed by the spec

- `SysfsRoot` defaults to `/sys` (the spec says `/`), so the fixture mirrors
  `/sys` as SPEC 16.1 lists it. `ALIENFAN_SYSFS_ROOT` and `ALIENFAN_CONFIG`
  exist for tests.
- Temperatures are `f64`, matching D-Bus `d` and TOML floats.
- The boot service does not use `systemd-udev-settle`; `apply --boot --wait`
  polls for the sysfs nodes instead.
- At boot a curve default is applied as its profile with boost 0; the daemon
  starts the curve after login.
- A curve is "in use" only when it is the active control of a default.
  The `curve` field of a default that uses another control does not block
  `DeleteCurve`.
- Curve ramps cap `dt` at 5 s, so a resume does not jump the fans. After a
  sensor failure the fan ramps up from 0; after an emergency it ramps down
  from 255.
- `override_until` is exposed in v1 through an additive D-Bus method,
  `SetDaemonOption(name, value)`, which accepts only `override_until`.
- The existing `/etc/tlp.d/50-alienware.conf` stays. `99-alienfan.conf` is
  read after it and blanks both values; uninstalling brings the 50 file back
  into effect.
- Serde type errors in the config (e.g. a string where a number goes) are
  reported in English; validation errors are in pt-BR.
