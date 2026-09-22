//! `alienfand`: per-user daemon that owns the fans and the thermal profile
//! at runtime and serves them on the session bus (SPEC 8).

mod service;
mod state;
mod watch;

use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use alienfan_core::SysfsRoot;
use alienfan_core::config::config_path;
use alienfan_proto::{BUS_NAME, OBJECT_PATH};
use anyhow::{Context, bail};
use tokio::signal::unix::{SignalKind, signal};
use tracing::{error, info, warn};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::prelude::*;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::object_server::InterfaceRef;

use crate::service::{Service, lock};
use crate::state::State;

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    init_logging();
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

/// journald under systemd, stderr otherwise. `ALIENFAN_DEBUG=1` logs every
/// sysfs write.
fn init_logging() {
    let level = if std::env::var_os("ALIENFAN_DEBUG").is_some() {
        LevelFilter::DEBUG
    } else {
        LevelFilter::INFO
    };
    let journald = std::env::var_os("JOURNAL_STREAM").and_then(|_| tracing_journald::layer().ok());
    let stderr = journald
        .is_none()
        .then(|| tracing_subscriber::fmt::layer().with_writer(std::io::stderr));
    tracing_subscriber::registry()
        .with(level)
        .with(journald)
        .with(stderr)
        .init();
}

async fn run() -> anyhow::Result<()> {
    let state = Arc::new(Mutex::new(State::new(
        SysfsRoot::from_env(),
        config_path(),
        Instant::now(),
    )));

    let conn = zbus::connection::Builder::session()
        .context("sem bus de sessão")?
        .serve_at(OBJECT_PATH, Service::new(state.clone()))?
        .build()
        .await
        .context("não foi possível conectar ao bus de sessão")?;
    let reply = conn
        .request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into())
        .await;
    match reply {
        Ok(RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner) => {}
        Ok(_) | Err(zbus::Error::NameTaken) => {
            bail!("{BUS_NAME} já tem dono: outro alienfand está rodando nesta sessão")
        }
        Err(e) => return Err(e.into()),
    }
    info!("publicado em {BUS_NAME}");

    let iface = conn
        .object_server()
        .interface::<_, Service>(OBJECT_PATH)
        .await?;
    tokio::spawn(watch::system_events(state.clone(), iface.clone()));

    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
    loop {
        let period = lock(&state).tick_period();
        tokio::select! {
            () = tokio::time::sleep(period) => {
                if let Err(e) = tick(&state, &iface).await {
                    warn!("falha ao publicar a telemetria: {e}");
                }
            }
            _ = term.recv() => break,
            _ = int.recv() => break,
        }
    }
    lock(&state).shutdown();
    Ok(())
}

async fn tick(state: &Mutex<State>, iface: &InterfaceRef<Service>) -> zbus::Result<()> {
    let (fans, temps) = {
        let mut s = lock(state);
        s.tick(Instant::now());
        s.telemetry()
    };
    Service::telemetry(iface.signal_emitter(), &fans, &temps).await?;
    iface.get().await.publish(iface.signal_emitter()).await
}
