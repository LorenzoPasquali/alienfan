//! System bus events (SPEC 8.3): `UPower` for the power source, `logind`
//! for resume. Both only trigger work that sysfs polling would do anyway, so the
//! daemon keeps working without them.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use tracing::{info, warn};
use zbus::object_server::InterfaceRef;

use crate::service::{Service, lock};
use crate::state::State;

/// logind needs a moment before the EC and the WMI driver answer again.
const RESUME_DELAY: Duration = Duration::from_secs(2);

#[zbus::proxy(
    interface = "org.freedesktop.UPower",
    default_service = "org.freedesktop.UPower",
    default_path = "/org/freedesktop/UPower",
    gen_blocking = false
)]
trait UPower {
    #[zbus(property)]
    fn on_battery(&self) -> zbus::Result<bool>;
}

#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1",
    gen_blocking = false
)]
trait Login1Manager {
    #[zbus(signal)]
    fn prepare_for_sleep(&self, start: bool) -> zbus::Result<()>;
}

pub async fn system_events(state: Arc<Mutex<State>>, iface: InterfaceRef<Service>) {
    if let Err(e) = watch(&state, &iface).await {
        warn!(
            "sem eventos do bus de sistema (UPower/logind): {e}; a fonte de energia segue por polling"
        );
    }
}

async fn watch(state: &Mutex<State>, iface: &InterfaceRef<Service>) -> zbus::Result<()> {
    let system = zbus::Connection::system().await?;
    let upower = UPowerProxy::new(&system).await?;
    let login = Login1ManagerProxy::new(&system).await?;
    let mut power = upower.receive_on_battery_changed().await;
    let mut sleep = login.receive_prepare_for_sleep().await?;
    info!("ouvindo UPower e logind");
    loop {
        tokio::select! {
            Some(_) = power.next() => lock(state).check_power(Instant::now()),
            Some(signal) = sleep.next() => {
                if *signal.args()?.start() {
                    info!("suspendendo");
                    continue;
                }
                tokio::time::sleep(RESUME_DELAY).await;
                lock(state).resume(Instant::now());
            }
            else => return Ok(()),
        }
        iface.get().await.publish(iface.signal_emitter()).await?;
    }
}
