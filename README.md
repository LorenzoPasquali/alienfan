# alienfan

Thermal profile and fan control for the Alienware 16 Aurora (AC16250) on
Zorin OS 18 / GNOME 46. It replaces the "boot Windows, tweak AWCC, come back"
routine. The full design is in [SPEC.md](SPEC.md).

## Status

| Milestone | What | State |
|---|---|---|
| M0 | Hardware validation (`docs/fase0.sh`, `docs/HARDWARE.md`) | run 1 done (battery); run 2 on AC pending |
| M1 | `alienfan-core`: model, config, curve engine, sysfs | done |
| M2 | `alienfan` CLI (direct mode), udev rule, boot service, TLP drop-in, installer | done |
| M3 | `alienfand` daemon over D-Bus (curve, power source, resume); CLI through it | done |
| M4 | GNOME Quick Settings extension | done |
| M5 | Tauri panel | frontend done and tested in a browser; Tauri side needs the apt packages to build |
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

When `alienfand` runs on the session bus, every command goes through it.
Otherwise the CLI writes sysfs directly and warns that nothing keeps the
setting across power changes; `control curve` then fails with code 5.
`apply` and `doctor` always act alone. With `ALIENFAN_SYSFS_ROOT` or
`ALIENFAN_CONFIG` set, the CLI never talks to the daemon, so a test tree
cannot reach the real one.

## Daemon

`alienfand` is a systemd user unit (`systemctl --user status alienfand`),
also started by D-Bus activation. Logs: `journalctl --user -u alienfand`
(`ALIENFAN_DEBUG=1` logs every sysfs write). The API is in
`crates/alienfan-proto/io.github.lorenzopasquali.AlienFan1.xml`; try it with

```bash
busctl --user introspect io.github.lorenzopasquali.AlienFan /io/github/lorenzopasquali/AlienFan
```

## GNOME extension

`gnome-extension/alienfan@lorenzopasquali.github.io`, for GNOME Shell 46
only (SPEC 2.4). The installer copies it to
`~/.local/share/gnome-shell/extensions/`; enable it with
`gnome-extensions enable alienfan@lorenzopasquali.github.io` after logging in
again. Its `dbus.js` embeds the interface XML; `cargo test -p alienfan-proto`
fails if the two copies differ.

The extension was checked in an isolated headless GNOME Shell 46 (own bus,
dconf and data dirs): it enables, disables and re-enables without JS errors,
with and without the daemon. Look for errors with

```bash
journalctl --user -b -o cat /usr/bin/gnome-shell | grep -i alienfan
```

## Panel

`panel/`: Tauri 2 with a TypeScript UI and no framework (Vite). It has
three tabs: Ventoinhas (animated fans, profile strip, boost), Curvas (SVG
editor with draggable points) and Padrões (AC and battery defaults). It only
talks to the daemon.

Building it needs the WebKitGTK development packages:

```bash
sudo apt install libwebkit2gtk-4.1-dev libdbus-1-dev libxdo-dev libssl-dev \
  libayatana-appindicator3-dev librsvg2-dev
cd panel && npm ci
npx tauri dev                  # window with hot reload, needs alienfand running
npx tauri build --no-bundle    # panel/src-tauri/target/release/alienfan-panel
```

The UI also runs in a plain browser against a simulated daemon, which is how
it was checked before the packages were installed:

```bash
cd panel && npm run dev        # http://localhost:5173
# ?theme=dark|light forces a theme; ?mock=offline|emergency|no-permission
# starts the simulation in that state.
```

`panel/src-tauri` is a Cargo workspace of its own, so the main workspace
builds and tests without WebKitGTK.

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
- API additions to SPEC 9 (additive, marked in the XML): `GetCurveOptions`,
  `SaveCurveWithOptions` (the panel edits hysteresis and ramps),
  `SetDaemonOption`, and the `OverrideUntil` and `EmergencyTempC` properties.
- `SetControl("curve", "")` runs the curve named by the current default.
- `SaveAsDefault` ends the override only when the target includes the
  current power source; saving the other source's default keeps it.
- `SetDefault` on the current source applies at once when no override is
  active, since the hardware follows that default.
- The daemon notices config edits by polling the file's mtime and size every
  tick instead of inotify: same effect, and it survives atomic renames.
- A profile change made outside alienfan (a key, `sudo`) is not fought: the
  daemon keeps the new profile and rewrites the boost, which the firmware
  reset.
- A config without a `[curves]` table uses the shipped curves; the first
  curve edit writes them into the file.
- The panel's profile hint recommends only Silencioso for less noise. The
  spec suggested "Silencioso ou Frio", but on this machine `cool` idles at
  ~4300 rpm (Phase 0), louder than `balanced`. Profiles are listed from
  quietest to loudest as measured.
- `override_until` is edited in the panel's Padrões tab (v1), through
  `SetDaemonOption`.
- Serde type errors in the config (e.g. a string where a number goes) are
  reported in English; validation errors are in pt-BR.
