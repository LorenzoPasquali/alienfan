//! Per-tick fan control: curve with ramps and hysteresis, emergency and
//! sensor failure handling (SPEC 8.4).
//!
//! Pure logic. The caller reads the sensors, passes the time in, and writes
//! the boosts it gets back; tests drive it with a fake clock.

use std::time::Duration;

use crate::curve::Curve;
use crate::model::{Boost, FanId, FanPair, Health};

/// Smoothing of the raw temperature.
pub const EMA_ALPHA: f64 = 0.35;
/// In curve mode, smaller changes than this are not written (except to
/// reach 0 or 255).
pub const WRITE_STEP: u8 = 2;
/// Consecutive failed reads before a fan goes back to the firmware.
pub const SENSOR_FAILURE_LIMIT: u32 = 3;
pub const EMERGENCY_EXIT_MARGIN_C: f64 = 5.0;
pub const EMERGENCY_EXIT_HOLD: Duration = Duration::from_secs(10);
/// Longest step fed to the ramps, so a suspend does not jump the fans.
const MAX_DT: Duration = Duration::from_secs(5);

/// Curve state of one fan.
#[derive(Debug, Clone)]
struct FanCurve {
    ema_c: Option<f64>,
    /// Fractional, so slow ramps accumulate across short ticks.
    current: f64,
    /// Smoothed temperature at the last rise. The fan only slows down once
    /// the temperature is `hysteresis_c` below it.
    last_rise_c: f64,
}

impl FanCurve {
    fn new(start: Boost) -> Self {
        Self {
            ema_c: None,
            current: f64::from(start.0),
            // No rise seen yet, so nothing holds the fan up.
            last_rise_c: f64::INFINITY,
        }
    }
}

/// Follows a curve with ramps and hysteresis.
#[derive(Debug, Clone)]
pub struct CurveEngine {
    curve: Curve,
    fans: FanPair<FanCurve>,
}

impl CurveEngine {
    /// `start` is the boost the fans have now, so switching to a curve
    /// ramps from there instead of jumping.
    pub fn new(curve: Curve, start: FanPair<Boost>) -> Self {
        Self {
            curve,
            fans: start.map(|_, b| FanCurve::new(b)),
        }
    }

    pub fn curve(&self) -> &Curve {
        &self.curve
    }

    /// Replaces the curve (e.g. after it was edited), keeping the state.
    pub fn set_curve(&mut self, curve: Curve) {
        self.curve = curve;
    }

    /// Returns the curve target (before ramps) and the new output.
    fn step(&mut self, fan: FanId, raw_c: f64, dt_s: f64) -> (Boost, f64) {
        let c = &self.curve;
        let s = &mut self.fans[fan];
        let t = s
            .ema_c
            .map_or(raw_c, |prev| EMA_ALPHA * raw_c + (1.0 - EMA_ALPHA) * prev);
        s.ema_c = Some(t);

        let target = c.target(fan, t);
        let goal = f64::from(target.0);
        if goal > s.current {
            s.current = goal.min(s.current + f64::from(c.ramp_up_per_s) * dt_s);
            s.last_rise_c = t;
        } else if goal < s.current && t <= s.last_rise_c - c.hysteresis_c {
            s.current = goal.max(s.current - f64::from(c.ramp_down_per_s) * dt_s);
        }
        (target, s.current)
    }

    /// After an emergency: start from full speed and ramp down normally.
    fn saturate(&mut self) {
        for fan in FanId::ALL.iter().copied() {
            let s = &mut self.fans[fan];
            s.current = f64::from(Boost::MAX.0);
            s.last_rise_c = f64::INFINITY;
        }
    }
}

/// Where the boost comes from.
#[derive(Debug, Clone)]
pub enum Mode {
    Firmware,
    Fixed(FanPair<Boost>),
    Curve(CurveEngine),
}

/// What to do after one tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TickOutput {
    /// Boosts to write now, in order.
    pub writes: Vec<(FanId, Boost)>,
    /// What each fan should run at, before ramps (`target_boost`).
    pub target: FanPair<Boost>,
    /// What each fan runs at after this tick.
    pub output: FanPair<Boost>,
    /// `Ok`, `Degraded` or `Emergency`.
    pub health: Health,
}

#[derive(Debug, Clone)]
struct Emergency {
    calm_since: Option<Duration>,
}

#[derive(Debug, Clone)]
pub struct Controller {
    mode: Mode,
    emergency_temp_c: f64,
    emergency: Option<Emergency>,
    failures: FanPair<u32>,
    target: FanPair<Boost>,
    output: FanPair<Boost>,
    written: FanPair<Option<Boost>>,
    last_tick: Option<Duration>,
}

impl Controller {
    /// `current` is the boost the hardware has now.
    pub fn new(mode: Mode, emergency_temp_c: f64, current: FanPair<Boost>) -> Self {
        Self {
            mode,
            emergency_temp_c,
            emergency: None,
            failures: FanPair::default(),
            target: current,
            output: current,
            written: current.map(|_, b| Some(b)),
            last_tick: None,
        }
    }

    pub fn mode(&self) -> &Mode {
        &self.mode
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    /// Switches to `curve`, ramping from the current output.
    pub fn set_curve(&mut self, curve: Curve) {
        self.mode = Mode::Curve(CurveEngine::new(curve, self.output));
    }

    pub fn set_emergency_temp(&mut self, temp_c: f64) {
        self.emergency_temp_c = temp_c;
    }

    pub fn in_emergency(&self) -> bool {
        self.emergency.is_some()
    }

    pub fn output(&self) -> FanPair<Boost> {
        self.output
    }

    /// Forgets what was written, so the next tick writes both fans. Call it
    /// after a profile change, a resume, or a failed write.
    pub fn invalidate(&mut self) {
        self.written = FanPair::both(None);
    }

    /// `temps` holds the configured sensor of each fan; `None` is a failed
    /// read. `now` is any monotonic clock.
    pub fn tick(&mut self, now: Duration, temps: FanPair<Option<f64>>) -> TickOutput {
        let dt_s = self
            .last_tick
            .map_or(Duration::ZERO, |last| now.saturating_sub(last).min(MAX_DT))
            .as_secs_f64();
        self.last_tick = Some(now);

        for fan in FanId::ALL.iter().copied() {
            self.failures[fan] = match temps[fan] {
                Some(_) => 0,
                None => self.failures[fan].saturating_add(1),
            };
        }
        let failed = self.failures.map(|_, n| n >= SENSOR_FAILURE_LIMIT);
        self.update_emergency(now, temps);

        for fan in FanId::ALL.iter().copied() {
            let (target, output) = match (&mut self.mode, temps[fan]) {
                (Mode::Firmware, _) => (Boost::MIN, Boost::MIN),
                (Mode::Fixed(boost), _) => (boost[fan], boost[fan]),
                (Mode::Curve(engine), Some(t)) => {
                    let (target, out) = engine.step(fan, t, dt_s);
                    (target, Boost::from_f64(out))
                }
                // The firmware takes over from a fan without a sensor. When
                // the sensor comes back, the curve ramps up from 0.
                (Mode::Curve(engine), None) if failed[fan] => {
                    engine.fans[fan] = FanCurve::new(Boost::MIN);
                    (Boost::MIN, Boost::MIN)
                }
                (Mode::Curve(_), None) => (self.target[fan], self.output[fan]),
            };
            self.target[fan] = target;
            self.output[fan] = output;
        }
        if self.emergency.is_some() {
            self.target = FanPair::both(Boost::MAX);
            self.output = FanPair::both(Boost::MAX);
        }

        let health = if self.emergency.is_some() {
            Health::Emergency
        } else if failed.cpu || failed.gpu {
            Health::Degraded
        } else {
            Health::Ok
        };
        TickOutput {
            writes: self.take_writes(),
            target: self.target,
            output: self.output,
            health,
        }
    }

    fn update_emergency(&mut self, now: Duration, temps: FanPair<Option<f64>>) {
        let limit = self.emergency_temp_c;
        let hot = temps.iter().any(|(_, t)| t.is_some_and(|t| t > limit));
        let calm = temps
            .iter()
            .all(|(_, t)| t.is_some_and(|t| t <= limit - EMERGENCY_EXIT_MARGIN_C));
        match &mut self.emergency {
            None if hot => self.emergency = Some(Emergency { calm_since: None }),
            None => {}
            Some(e) if calm => {
                let since = *e.calm_since.get_or_insert(now);
                if now.saturating_sub(since) >= EMERGENCY_EXIT_HOLD {
                    self.emergency = None;
                    if let Mode::Curve(engine) = &mut self.mode {
                        engine.saturate();
                    }
                }
            }
            Some(e) => e.calm_since = None,
        }
    }

    fn take_writes(&mut self) -> Vec<(FanId, Boost)> {
        let curve = matches!(self.mode, Mode::Curve(_)) && self.emergency.is_none();
        let mut writes = Vec::new();
        for fan in FanId::ALL.iter().copied() {
            let want = self.output[fan];
            let write = match self.written[fan] {
                None => true,
                Some(had) if curve => {
                    had != want
                        && (had.0.abs_diff(want.0) >= WRITE_STEP
                            || want == Boost::MIN
                            || want == Boost::MAX)
                }
                Some(had) => had != want,
            };
            if write {
                self.written[fan] = Some(want);
                writes.push((fan, want));
            }
        }
        writes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::CurvePoint;

    /// 40 °C → 0, 80 °C → 200; +40/s up, −10/s down, hysteresis 3 °C.
    fn curve() -> Curve {
        let points = vec![CurvePoint::new(40.0, 0), CurvePoint::new(80.0, 200)];
        Curve::new(FanPair::both(points), 3.0, 40, 10).unwrap()
    }

    fn curve_controller() -> Controller {
        let start = FanPair::both(Boost::MIN);
        Controller::new(Mode::Curve(CurveEngine::new(curve(), start)), 95.0, start)
    }

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    /// Runs one tick per second from `start`, with the same temperature on
    /// both fans. Returns every output.
    fn run(c: &mut Controller, start: u64, temps: &[f64]) -> Vec<TickOutput> {
        temps
            .iter()
            .zip(start..)
            .map(|(&t, s)| c.tick(secs(s), FanPair::both(Some(t))))
            .collect()
    }

    fn cpu(outs: &[TickOutput]) -> Vec<u8> {
        outs.iter().map(|o| o.output.cpu.0).collect()
    }

    #[test]
    fn ramps_up_at_the_configured_rate() {
        let mut c = curve_controller();
        // First tick has dt = 0: nothing moves yet. Target at 80 °C is 200.
        let outs = run(&mut c, 0, &[80.0; 7]);
        assert_eq!(outs[0].target.cpu, Boost(200));
        assert_eq!(cpu(&outs), [0, 40, 80, 120, 160, 200, 200]);
    }

    #[test]
    fn hysteresis_holds_then_ramps_down_slowly() {
        let mut c = curve_controller();
        run(&mut c, 0, &[80.0; 6]); // reaches 200, last rise at 80 °C
        // EMA of a step: 80 → 79.65 → ... stays above 77 °C for a while.
        let outs = run(&mut c, 6, &[79.0; 3]);
        assert_eq!(cpu(&outs), [200, 200, 200], "within hysteresis");
        // Far below: ramps down at 10/s, not straight to the target.
        let outs = run(&mut c, 9, &[40.0; 40]);
        let out = cpu(&outs);
        assert!(
            out.windows(2).all(|w| w[0] >= w[1] && w[0] - w[1] <= 10),
            "{out:?}"
        );
        assert!(out[0] >= 190, "{out:?}");
        assert_eq!(*out.last().unwrap(), 0);
    }

    #[test]
    fn small_changes_are_not_written() {
        let mut c = curve_controller();
        // Target 5 at 41 °C: 0 → 5 in one step (40/s), written.
        let outs = run(&mut c, 0, &[41.0, 41.0]);
        assert_eq!(
            outs[1].writes,
            [(FanId::Cpu, Boost(5)), (FanId::Gpu, Boost(5))]
        );
        // 41.4 °C (smoothed ≈ 41.1) → target 6: a change of 1 is skipped.
        let outs = run(&mut c, 2, &[41.4, 41.4]);
        assert!(outs.iter().all(|o| o.writes.is_empty()), "{outs:?}");
        assert_eq!(outs[1].output.cpu, Boost(6));
        // A change of 2 or more is written.
        let outs = run(&mut c, 4, &[43.0; 3]);
        assert!(outs.iter().any(|o| !o.writes.is_empty()));
    }

    #[test]
    fn reaching_zero_is_always_written() {
        let start = FanPair::both(Boost(1));
        let mut c = Controller::new(Mode::Curve(CurveEngine::new(curve(), start)), 95.0, start);
        let outs = run(&mut c, 0, &[20.0, 20.0]);
        assert_eq!(
            outs[1].writes,
            [(FanId::Cpu, Boost(0)), (FanId::Gpu, Boost(0))]
        );
    }

    #[test]
    fn emergency_enters_and_exits_after_hold() {
        let mut c = curve_controller();
        let outs = run(&mut c, 0, &[50.0, 96.0]);
        assert_eq!(outs[1].health, Health::Emergency);
        assert_eq!(outs[1].output, FanPair::both(Boost::MAX));
        assert_eq!(
            outs[1].writes,
            [(FanId::Cpu, Boost::MAX), (FanId::Gpu, Boost::MAX)]
        );

        // 91 °C is not 5 °C below 95: still in emergency.
        let outs = run(&mut c, 2, &[91.0; 20]);
        assert!(outs.iter().all(|o| o.health == Health::Emergency));

        // 90 °C for 10 s: leaves on the tick where the hold completes.
        let outs = run(&mut c, 22, &[90.0; 12]);
        let first_ok = outs.iter().position(|o| o.health == Health::Ok).unwrap();
        assert_eq!(first_ok, 10, "calm from t=22, leaves at t=32");
        // Back on the curve, ramping down from 255 instead of jumping.
        let after = cpu(&outs[first_ok..]);
        assert!(after[0] >= 245, "{after:?}");
    }

    #[test]
    fn a_hot_tick_resets_the_exit_hold() {
        let mut c = curve_controller();
        run(&mut c, 0, &[96.0]);
        run(&mut c, 1, &[80.0; 8]);
        run(&mut c, 9, &[93.0]); // not calm: restart the 10 s
        let outs = run(&mut c, 10, &[80.0; 10]);
        assert!(outs.iter().all(|o| o.health == Health::Emergency));
        let outs = run(&mut c, 20, &[80.0]);
        assert_eq!(outs[0].health, Health::Ok);
    }

    #[test]
    fn emergency_applies_to_every_mode() {
        for mode in [Mode::Firmware, Mode::Fixed(FanPair::both(Boost(10)))] {
            let mut c = Controller::new(mode, 95.0, FanPair::default());
            let out = c.tick(secs(0), FanPair::new(Some(50.0), Some(100.0)));
            assert_eq!(out.health, Health::Emergency);
            assert_eq!(out.output, FanPair::both(Boost::MAX));
        }
    }

    #[test]
    fn sensor_failure_hands_the_fan_to_firmware_after_three_reads() {
        let mut c = curve_controller();
        run(&mut c, 0, &[80.0; 4]); // cpu at 120
        let fail = |c: &mut Controller, s| c.tick(secs(s), FanPair::new(None, Some(80.0)));
        let o1 = fail(&mut c, 4);
        let o2 = fail(&mut c, 5);
        assert_eq!(
            (o1.output.cpu, o2.output.cpu),
            (Boost(120), Boost(120)),
            "holds"
        );
        assert_eq!(o2.health, Health::Ok);
        let o3 = fail(&mut c, 6);
        assert_eq!(o3.output.cpu, Boost::MIN);
        assert_eq!(o3.health, Health::Degraded);
        assert!(o3.writes.contains(&(FanId::Cpu, Boost::MIN)));
        // The GPU keeps following its curve.
        assert_eq!(o3.output.gpu, Boost(200));
        // Recovers on the next good read, ramping up from 0.
        let o4 = c.tick(secs(7), FanPair::both(Some(80.0)));
        assert_eq!(o4.health, Health::Ok);
        assert_eq!(o4.output.cpu, Boost(40));
    }

    #[test]
    fn fixed_and_firmware_write_only_on_change() {
        let mut c = Controller::new(
            Mode::Fixed(FanPair::new(Boost(100), Boost(50))),
            95.0,
            FanPair::default(),
        );
        let o = c.tick(secs(0), FanPair::both(Some(50.0)));
        assert_eq!(
            o.writes,
            [(FanId::Cpu, Boost(100)), (FanId::Gpu, Boost(50))]
        );
        assert!(c.tick(secs(1), FanPair::both(Some(50.0))).writes.is_empty());
        c.set_mode(Mode::Firmware);
        let o = c.tick(secs(2), FanPair::both(Some(50.0)));
        assert_eq!(o.writes, [(FanId::Cpu, Boost(0)), (FanId::Gpu, Boost(0))]);
    }

    #[test]
    fn invalidate_rewrites_both_fans() {
        let mut c = Controller::new(Mode::Firmware, 95.0, FanPair::default());
        assert!(c.tick(secs(0), FanPair::both(Some(50.0))).writes.is_empty());
        c.invalidate();
        assert_eq!(c.tick(secs(1), FanPair::both(Some(50.0))).writes.len(), 2);
    }

    #[test]
    fn switching_to_a_curve_ramps_from_the_current_output() {
        let mut c = Controller::new(
            Mode::Fixed(FanPair::both(Boost(200))),
            95.0,
            FanPair::default(),
        );
        c.tick(secs(0), FanPair::both(Some(40.0)));
        c.set_curve(curve());
        // Target at 40 °C is 0: ramps down at 10/s from 200.
        let outs = run(&mut c, 1, &[40.0; 3]);
        assert_eq!(cpu(&outs), [190, 180, 170]);
    }

    #[test]
    fn long_gaps_are_capped() {
        let mut c = curve_controller();
        run(&mut c, 0, &[80.0]);
        // 60 s later (e.g. after suspend): at most 5 s of ramp.
        let o = c.tick(secs(60), FanPair::both(Some(80.0)));
        assert_eq!(o.output.cpu, Boost(200));
        let mut c = curve_controller();
        run(&mut c, 0, &[60.0]);
        let o = c.tick(secs(60), FanPair::both(Some(60.0)));
        assert_eq!(
            o.output.cpu,
            Boost(100),
            "target 100 reached within 5 s at 40/s"
        );
    }
}
