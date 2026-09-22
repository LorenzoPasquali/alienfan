//! `alienfan-panel`: the Tauri side of the panel (SPEC 13.1).
//!
//! Every command forwards to `alienfand` over the session bus; the UI never
//! touches sysfs. A background task relays the `Telemetry` signal as the
//! `telemetry` event, property changes as `state`, and connection changes
//! as `daemon`, reconnecting every 2 s.

use std::collections::HashMap;
use std::time::Duration;

use alienfan_proto::{
    AlienFan1Proxy, BUS_NAME, CurveOptions, FanTelemetry, OBJECT_PATH, PresetDict, error_message,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::RwLock;
use zbus::proxy::CacheProperties;

const RECONNECT: Duration = Duration::from_secs(2);
const NOT_RUNNING: &str = "o serviço alienfand não está rodando";

type CmdResult<T = ()> = Result<T, String>;

/// The daemon proxy while the daemon is on the bus.
#[derive(Default)]
struct Daemon {
    proxy: RwLock<Option<AlienFan1Proxy<'static>>>,
}

impl Daemon {
    async fn get(&self) -> CmdResult<AlienFan1Proxy<'static>> {
        self.proxy
            .read()
            .await
            .clone()
            .ok_or_else(|| NOT_RUNNING.to_owned())
    }
}

#[allow(clippy::needless_pass_by_value)] // shaped for `map_err`
fn message(e: zbus::Error) -> String {
    error_message(&e)
}

// ---------- JSON shapes of src/api.ts ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DaemonState {
    version: String,
    health: String,
    health_message: String,
    power_source: String,
    profile: String,
    available_profiles: Vec<String>,
    control: String,
    active_curve: String,
    override_active: bool,
    boost_requires_custom: bool,
    override_until: String,
    emergency_temp_c: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Fan {
    id: String,
    label: String,
    rpm: u32,
    rpm_max: u32,
    boost: u8,
    target_boost: u8,
    temp_c: Option<f64>,
    sensor: String,
}

#[derive(Serialize)]
struct Telemetry {
    fans: Vec<Fan>,
    temps: HashMap<String, f64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Preset {
    profile: String,
    control: String,
    fixed_cpu: u8,
    fixed_gpu: u8,
    curve: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PresetPatch {
    profile: Option<String>,
    control: Option<String>,
    fixed_cpu: Option<u8>,
    fixed_gpu: Option<u8>,
    curve: Option<String>,
}

#[derive(Serialize)]
struct Defaults {
    ac: Preset,
    battery: Preset,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Curve {
    cpu: Vec<(f64, u8)>,
    gpu: Vec<(f64, u8)>,
    hysteresis_c: f64,
    ramp_up_per_s: u16,
    ramp_down_per_s: u16,
}

async fn read_state(p: &AlienFan1Proxy<'_>) -> zbus::Result<DaemonState> {
    Ok(DaemonState {
        version: p.version().await?,
        health: p.health().await?,
        health_message: p.health_message().await?,
        power_source: p.power_source().await?,
        profile: p.profile().await?,
        available_profiles: p.available_profiles().await?,
        control: p.control().await?,
        active_curve: p.active_curve().await?,
        override_active: p.override_active().await?,
        boost_requires_custom: p.boost_requires_custom().await?,
        override_until: p.override_until().await?,
        emergency_temp_c: p.emergency_temp_c().await?,
    })
}

fn telemetry(fans: Vec<FanTelemetry>, temps: HashMap<String, f64>) -> Telemetry {
    Telemetry {
        fans: fans
            .into_iter()
            .map(|f| Fan {
                id: f.id,
                label: f.label,
                rpm: f.rpm,
                rpm_max: f.rpm_max,
                boost: f.boost,
                target_boost: f.target_boost,
                temp_c: f.temp_c,
                sensor: f.sensor,
            })
            .collect(),
        temps,
    }
}

fn preset(d: PresetDict) -> Preset {
    Preset {
        profile: d.profile.unwrap_or_default(),
        control: d.control.unwrap_or_default(),
        fixed_cpu: d.fixed_cpu.unwrap_or_default(),
        fixed_gpu: d.fixed_gpu.unwrap_or_default(),
        curve: d.curve.unwrap_or_default(),
    }
}

// ---------- Commands ----------

#[tauri::command]
async fn get_state(d: State<'_, Daemon>) -> CmdResult<Option<DaemonState>> {
    let Some(p) = d.proxy.read().await.clone() else {
        return Ok(None);
    };
    // A daemon that just left looks like "not running", not like an error.
    Ok(read_state(&p).await.ok())
}

#[tauri::command]
async fn get_telemetry(d: State<'_, Daemon>) -> CmdResult<Telemetry> {
    let (fans, temps) = d.get().await?.get_telemetry().await.map_err(message)?;
    Ok(telemetry(fans, temps))
}

#[tauri::command]
async fn get_defaults(d: State<'_, Daemon>) -> CmdResult<Defaults> {
    let (ac, battery) = d.get().await?.get_defaults().await.map_err(message)?;
    Ok(Defaults {
        ac: preset(ac),
        battery: preset(battery),
    })
}

#[tauri::command]
async fn list_curves(d: State<'_, Daemon>) -> CmdResult<Vec<String>> {
    d.get().await?.list_curves().await.map_err(message)
}

#[tauri::command]
async fn get_curve(name: String, d: State<'_, Daemon>) -> CmdResult<Curve> {
    let p = d.get().await?;
    let options = p.get_curve_options(&name).await.map_err(message)?;
    Ok(Curve {
        cpu: p.get_curve(&name, "cpu").await.map_err(message)?,
        gpu: p.get_curve(&name, "gpu").await.map_err(message)?,
        hysteresis_c: options.hysteresis_c.unwrap_or_default(),
        ramp_up_per_s: options.ramp_up_per_s.unwrap_or_default(),
        ramp_down_per_s: options.ramp_down_per_s.unwrap_or_default(),
    })
}

#[tauri::command]
async fn set_profile(profile: String, d: State<'_, Daemon>) -> CmdResult {
    d.get().await?.set_profile(&profile).await.map_err(message)
}

#[tauri::command]
async fn set_fixed_boost(fan: String, boost: u8, d: State<'_, Daemon>) -> CmdResult {
    d.get()
        .await?
        .set_fixed_boost(&fan, boost)
        .await
        .map_err(message)
}

#[tauri::command]
async fn set_control(control: String, curve: String, d: State<'_, Daemon>) -> CmdResult {
    d.get()
        .await?
        .set_control(&control, &curve)
        .await
        .map_err(message)
}

#[tauri::command]
async fn restore_default(d: State<'_, Daemon>) -> CmdResult {
    d.get().await?.restore_default().await.map_err(message)
}

#[tauri::command]
async fn save_as_default(target: String, d: State<'_, Daemon>) -> CmdResult {
    d.get()
        .await?
        .save_as_default(&target)
        .await
        .map_err(message)
}

#[tauri::command]
async fn set_default(target: String, preset: PresetPatch, d: State<'_, Daemon>) -> CmdResult {
    let dict = PresetDict {
        profile: preset.profile,
        control: preset.control,
        fixed_cpu: preset.fixed_cpu,
        fixed_gpu: preset.fixed_gpu,
        curve: preset.curve,
    };
    d.get()
        .await?
        .set_default(&target, &dict)
        .await
        .map_err(message)
}

#[tauri::command]
async fn save_curve(name: String, curve: Curve, d: State<'_, Daemon>) -> CmdResult {
    let options = CurveOptions {
        hysteresis_c: Some(curve.hysteresis_c),
        ramp_up_per_s: Some(curve.ramp_up_per_s),
        ramp_down_per_s: Some(curve.ramp_down_per_s),
    };
    d.get()
        .await?
        .save_curve_with_options(&name, &curve.cpu, &curve.gpu, &options)
        .await
        .map_err(message)
}

#[tauri::command]
async fn delete_curve(name: String, d: State<'_, Daemon>) -> CmdResult {
    d.get().await?.delete_curve(&name).await.map_err(message)
}

#[tauri::command]
async fn set_override_until(value: String, d: State<'_, Daemon>) -> CmdResult {
    let value = zbus::zvariant::Value::from(value.as_str());
    d.get()
        .await?
        .set_daemon_option("override_until", &value)
        .await
        .map_err(message)
}

#[tauri::command]
async fn start_daemon() -> CmdResult {
    let out = tokio::process::Command::new("systemctl")
        .args(["--user", "start", "alienfand.service"])
        .output()
        .await
        .map_err(|e| format!("não foi possível rodar o systemctl: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "não foi possível iniciar o serviço: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

// ---------- Daemon connection ----------

/// Finds the daemon, relays its events while it stays, and starts over.
async fn watch_daemon(app: AppHandle) {
    loop {
        match zbus::Connection::session().await {
            Ok(conn) => {
                if let Err(e) = serve(&app, &conn).await {
                    eprintln!("alienfan-panel: {e}");
                }
            }
            Err(e) => eprintln!("alienfan-panel: sem bus de sessão: {e}"),
        }
        tokio::time::sleep(RECONNECT).await;
    }
}

/// While the connection lives: wait for the daemon, relay, repeat.
async fn serve(app: &AppHandle, conn: &zbus::Connection) -> zbus::Result<()> {
    let dbus = zbus::fdo::DBusProxy::new(conn).await?;
    loop {
        // Never start the daemon by D-Bus activation from here: a daemon
        // the user stopped stays stopped until "Iniciar serviço".
        if dbus.name_has_owner(BUS_NAME.try_into()?).await? {
            let proxy = AlienFan1Proxy::builder(conn)
                .cache_properties(CacheProperties::No)
                .build()
                .await?;
            *app.state::<Daemon>().proxy.write().await = Some(proxy.clone());
            let _ = app.emit("daemon", serde_json::json!({ "connected": true }));
            if let Err(e) = relay(app, conn, &proxy).await {
                eprintln!("alienfan-panel: {e}");
            }
            *app.state::<Daemon>().proxy.write().await = None;
            let _ = app.emit("daemon", serde_json::json!({ "connected": false }));
        }
        tokio::time::sleep(RECONNECT).await;
    }
}

async fn relay(
    app: &AppHandle,
    conn: &zbus::Connection,
    proxy: &AlienFan1Proxy<'static>,
) -> zbus::Result<()> {
    let props = zbus::fdo::PropertiesProxy::builder(conn)
        .destination(BUS_NAME)?
        .path(OBJECT_PATH)?
        .build()
        .await?;
    let mut changes = props.receive_properties_changed().await?;
    let mut signals = proxy.receive_telemetry().await?;
    let mut owner = proxy.inner().receive_owner_changed().await?;
    if let Ok(state) = read_state(proxy).await {
        let _ = app.emit("state", state);
    }
    loop {
        tokio::select! {
            Some(signal) = signals.next() => {
                if let Ok(args) = signal.args() {
                    let payload = telemetry(args.fans().clone(), args.temps().clone());
                    let _ = app.emit("telemetry", payload);
                }
            }
            Some(_) = changes.next() => {
                if let Ok(state) = read_state(proxy).await {
                    let _ = app.emit("state", state);
                }
            }
            Some(new_owner) = owner.next() => {
                if new_owner.is_none() {
                    return Ok(());
                }
            }
            else => return Ok(()),
        }
    }
}

fn main() {
    tauri::Builder::default()
        // Opening the panel again focuses the window that is already open.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .manage(Daemon::default())
        .setup(|app| {
            tauri::async_runtime::spawn(watch_daemon(app.handle().clone()));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            get_telemetry,
            get_defaults,
            list_curves,
            get_curve,
            set_profile,
            set_fixed_boost,
            set_control,
            restore_default,
            save_as_default,
            set_default,
            save_curve,
            delete_curve,
            set_override_until,
            start_daemon,
        ])
        .run(tauri::generate_context!())
        .expect("error while running alienfan-panel");
}
