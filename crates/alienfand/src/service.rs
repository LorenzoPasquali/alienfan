//! The D-Bus face of [`State`] (SPEC 9).

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use alienfan_proto::{CurveOptions, FanTelemetry, Points, PresetDict, Temps};
use zbus::object_server::SignalEmitter;
use zvariant::Value;

use crate::state::{Props, Result, State};

pub struct Service {
    state: Arc<Mutex<State>>,
    /// Last published properties: getters read them, `publish` diffs them.
    props: Mutex<Props>,
}

impl Service {
    pub fn new(state: Arc<Mutex<State>>) -> Self {
        let props = lock(&state).props();
        Self {
            state,
            props: Mutex::new(props),
        }
    }

    fn props(&self) -> Props {
        lock(&self.props).clone()
    }

    /// Emits `PropertiesChanged` for every property that changed.
    #[allow(clippy::float_cmp)] // any change of the value is a change
    pub async fn publish(&self, emitter: &SignalEmitter<'_>) -> zbus::Result<()> {
        let new = lock(&self.state).props();
        let old = std::mem::replace(&mut *lock(&self.props), new.clone());
        macro_rules! emit {
            ($($field:ident => $changed:ident),+ $(,)?) => {
                $(if old.$field != new.$field {
                    self.$changed(emitter).await?;
                })+
            };
        }
        emit!(
            health => health_changed,
            health_message => health_message_changed,
            power_source => power_source_changed,
            profile => profile_changed,
            available_profiles => available_profiles_changed,
            control => control_changed,
            active_curve => active_curve_changed,
            override_active => override_active_changed,
            boost_requires_custom => boost_requires_custom_changed,
            override_until => override_until_changed,
            emergency_temp_c => emergency_temp_c_changed,
        );
        Ok(())
    }

    /// Runs a state change, then publishes whatever it changed.
    async fn change(
        &self,
        emitter: &SignalEmitter<'_>,
        f: impl FnOnce(&mut State, Instant) -> Result<()>,
    ) -> Result<()> {
        let result = f(&mut lock(&self.state), Instant::now());
        self.publish(emitter).await?;
        result
    }
}

/// A panic while holding the lock leaves the state usable: every change is
/// applied whole or not at all.
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[zbus::interface(name = "io.github.lorenzopasquali.AlienFan1")]
impl Service {
    fn get_telemetry(&self) -> (Vec<FanTelemetry>, Temps) {
        lock(&self.state).telemetry()
    }

    fn get_defaults(&self) -> (PresetDict, PresetDict) {
        lock(&self.state).get_defaults()
    }

    fn list_curves(&self) -> Vec<String> {
        lock(&self.state).list_curves()
    }

    fn get_curve(&self, name: &str, fan: &str) -> Result<Points> {
        lock(&self.state).get_curve(name, fan)
    }

    fn get_curve_options(&self, name: &str) -> Result<CurveOptions> {
        lock(&self.state).get_curve_options(name)
    }

    async fn set_profile(
        &self,
        profile: &str,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
    ) -> Result<()> {
        self.change(&e, |s, now| s.set_profile(profile, now)).await
    }

    async fn set_fixed_boost(
        &self,
        fan: &str,
        boost: u8,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
    ) -> Result<()> {
        self.change(&e, |s, now| s.set_fixed_boost(fan, boost, now))
            .await
    }

    async fn set_control(
        &self,
        control: &str,
        curve: &str,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
    ) -> Result<()> {
        self.change(&e, |s, now| s.set_control(control, curve, now))
            .await
    }

    async fn restore_default(&self, #[zbus(signal_emitter)] e: SignalEmitter<'_>) -> Result<()> {
        self.change(&e, State::restore_default).await
    }

    async fn save_as_default(
        &self,
        target: &str,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
    ) -> Result<()> {
        self.change(&e, |s, now| s.save_as_default(target, now))
            .await
    }

    async fn set_default(
        &self,
        target: &str,
        preset: PresetDict,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
    ) -> Result<()> {
        self.change(&e, |s, now| s.set_default(target, &preset, now))
            .await
    }

    async fn save_curve(
        &self,
        name: &str,
        cpu: Points,
        gpu: Points,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
    ) -> Result<()> {
        self.change(&e, |s, _| {
            s.save_curve(name, &cpu, &gpu, &CurveOptions::default())
        })
        .await
    }

    async fn save_curve_with_options(
        &self,
        name: &str,
        cpu: Points,
        gpu: Points,
        options: CurveOptions,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
    ) -> Result<()> {
        self.change(&e, |s, _| s.save_curve(name, &cpu, &gpu, &options))
            .await
    }

    async fn delete_curve(
        &self,
        name: &str,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
    ) -> Result<()> {
        self.change(&e, |s, _| s.delete_curve(name)).await
    }

    async fn reload_config(&self, #[zbus(signal_emitter)] e: SignalEmitter<'_>) -> Result<()> {
        self.change(&e, State::reload).await
    }

    async fn set_daemon_option(
        &self,
        name: &str,
        value: Value<'_>,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
    ) -> Result<()> {
        self.change(&e, |s, _| s.set_daemon_option(name, &value))
            .await
    }

    #[zbus(signal)]
    pub async fn telemetry(
        emitter: &SignalEmitter<'_>,
        fans: &[FanTelemetry],
        temps: &Temps,
    ) -> zbus::Result<()>;

    #[zbus(property)]
    #[allow(clippy::unused_self)]
    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    #[zbus(property)]
    fn health(&self) -> String {
        self.props().health
    }

    #[zbus(property)]
    fn health_message(&self) -> String {
        self.props().health_message
    }

    #[zbus(property)]
    fn power_source(&self) -> String {
        self.props().power_source
    }

    #[zbus(property)]
    fn profile(&self) -> String {
        self.props().profile
    }

    #[zbus(property)]
    fn available_profiles(&self) -> Vec<String> {
        self.props().available_profiles
    }

    #[zbus(property)]
    fn control(&self) -> String {
        self.props().control
    }

    #[zbus(property)]
    fn active_curve(&self) -> String {
        self.props().active_curve
    }

    #[zbus(property)]
    fn override_active(&self) -> bool {
        self.props().override_active
    }

    #[zbus(property)]
    fn boost_requires_custom(&self) -> bool {
        self.props().boost_requires_custom
    }

    #[zbus(property)]
    fn override_until(&self) -> String {
        self.props().override_until
    }

    #[zbus(property, name = "EmergencyTempC")]
    fn emergency_temp_c(&self) -> f64 {
        self.props().emergency_temp_c
    }
}
