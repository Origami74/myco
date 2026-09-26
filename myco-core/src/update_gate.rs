//! When an update check may run — the one throttle every trigger shares
//! (`docs/design/nsite/nsite-updates.md` §3.1).
//!
//! Kotlin asks for a check when the app comes to the foreground and on a slow
//! timer while the process lives; the user asks through "Check for updates".
//! All of them land here, so the throttle and the no-overlap rule hold no matter
//! how many triggers fire at once. The decision is a pure function of the gate
//! and a clock reading, so it is tested without a runtime.

/// Who asked for an update check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckTrigger {
    /// The user tapped "Check for updates": never throttled, and its result is
    /// reported (the one-shot toast).
    Manual,
    /// Foreground or periodic: throttled by [`AUTO_MIN_INTERVAL_SECS`], and
    /// silent — updates apply, but no toast unless a manual press joins it.
    Auto,
}

/// How long after a check started an automatic one is skipped. Coming to the
/// foreground happens on every app switch; this caps the automatic load on
/// public relays at two checks an hour, while a phone picked up after a
/// meeting still checks on the first open.
pub const AUTO_MIN_INTERVAL_SECS: u64 = 30 * 60;

/// What [`UpdateGate::begin`] decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Start a check now; call [`UpdateGate::finish`] when it ends.
    Run,
    /// A check is already running; none is started. A manual press marks the
    /// running one to report its result instead.
    AlreadyRunning,
    /// An automatic check ran too recently; none is started.
    TooSoon,
}

/// The throttle's state. Times are wall-clock unix seconds rather than a
/// monotonic clock: Android's monotonic clock stops while the phone sleeps, so
/// a check before bed would still read as "minutes ago" in the morning.
#[derive(Debug, Default)]
pub struct UpdateGate {
    in_flight: bool,
    /// When the last check that ran started.
    last_started: Option<u64>,
    /// Whether the running check reports its result.
    report: bool,
}

impl UpdateGate {
    /// Decide without changing anything.
    pub fn decide(&self, trigger: CheckTrigger, now: u64) -> Decision {
        if self.in_flight {
            return Decision::AlreadyRunning;
        }
        if trigger == CheckTrigger::Auto && !self.auto_due(now) {
            return Decision::TooSoon;
        }
        Decision::Run
    }

    /// Decide, and record it: a `Run` marks a check in flight.
    pub fn begin(&mut self, trigger: CheckTrigger, now: u64) -> Decision {
        let decision = self.decide(trigger, now);
        match decision {
            Decision::Run => {
                self.in_flight = true;
                self.last_started = Some(now);
                self.report = trigger == CheckTrigger::Manual;
            }
            Decision::AlreadyRunning if trigger == CheckTrigger::Manual => self.report = true,
            Decision::AlreadyRunning | Decision::TooSoon => {}
        }
        decision
    }

    /// The running check ended. Returns whether it reports its result.
    pub fn finish(&mut self) -> bool {
        self.in_flight = false;
        std::mem::take(&mut self.report)
    }

    fn auto_due(&self, now: u64) -> bool {
        match self.last_started {
            None => true,
            // A clock set backwards reads as due rather than stalling checks
            // until it catches up again.
            Some(last) => now
                .checked_sub(last)
                .is_none_or(|elapsed| elapsed >= AUTO_MIN_INTERVAL_SECS),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use CheckTrigger::{Auto, Manual};

    const T0: u64 = 1_800_000_000;

    #[test]
    fn first_auto_check_runs() {
        let mut g = UpdateGate::default();
        assert_eq!(g.begin(Auto, T0), Decision::Run);
    }

    #[test]
    fn auto_is_throttled_after_any_check() {
        let mut g = UpdateGate::default();
        assert_eq!(g.begin(Manual, T0), Decision::Run);
        g.finish();
        assert_eq!(g.begin(Auto, T0 + 60), Decision::TooSoon);
        assert_eq!(
            g.begin(Auto, T0 + AUTO_MIN_INTERVAL_SECS - 1),
            Decision::TooSoon
        );
        assert_eq!(g.begin(Auto, T0 + AUTO_MIN_INTERVAL_SECS), Decision::Run);
    }

    #[test]
    fn manual_bypasses_the_throttle() {
        let mut g = UpdateGate::default();
        assert_eq!(g.begin(Auto, T0), Decision::Run);
        g.finish();
        assert_eq!(g.begin(Manual, T0 + 1), Decision::Run);
    }

    #[test]
    fn nothing_overlaps_a_running_check() {
        let mut g = UpdateGate::default();
        assert_eq!(g.begin(Auto, T0), Decision::Run);
        let later = T0 + 10 * AUTO_MIN_INTERVAL_SECS;
        assert_eq!(g.begin(Auto, later), Decision::AlreadyRunning);
        assert_eq!(g.begin(Manual, later), Decision::AlreadyRunning);
        g.finish();
        assert_eq!(g.begin(Manual, later), Decision::Run);
    }

    #[test]
    fn auto_check_is_silent_unless_manual_joins() {
        let mut g = UpdateGate::default();
        g.begin(Auto, T0);
        assert!(!g.finish());

        g.begin(Auto, T0 + AUTO_MIN_INTERVAL_SECS);
        g.begin(Manual, T0 + AUTO_MIN_INTERVAL_SECS + 1);
        assert!(
            g.finish(),
            "a manual press waits on the running check's result"
        );
    }

    #[test]
    fn manual_check_reports_once() {
        let mut g = UpdateGate::default();
        g.begin(Manual, T0);
        assert!(g.finish());
        g.begin(Manual, T0 + 1);
        g.finish();
        g.begin(Auto, T0 + AUTO_MIN_INTERVAL_SECS + 1);
        assert!(!g.finish(), "report does not leak into the next check");
    }

    #[test]
    fn clock_set_backwards_reads_as_due() {
        let mut g = UpdateGate::default();
        g.begin(Auto, T0);
        g.finish();
        assert_eq!(g.decide(Auto, T0 - 3600), Decision::Run);
    }

    #[test]
    fn decide_does_not_change_state() {
        let g = UpdateGate::default();
        assert_eq!(g.decide(Auto, T0), Decision::Run);
        assert_eq!(g.decide(Auto, T0), Decision::Run);
    }
}
