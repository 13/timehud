//! The HUD loop's timers and the `poll(2)` timeout they add up to.

/// Config writes are debounced this long after the last `SaveSoon`.
pub const SAVE_DEBOUNCE: f64 = 0.3;
/// `Sound::idle_release` cadence.
pub const IDLE_RELEASE: f64 = 1.0;
/// How long the IPC listener stays out of the poll set after `accept`
/// failed for a reason other than "nothing pending" (e.g. `EMFILE`): the
/// pending connection keeps the listener readable, so polling it would
/// spin the loop.
pub const ACCEPT_BACKOFF: f64 = 1.0;

/// When the loop must wake up next, per reason (seconds on the HUD clock).
#[derive(Debug, Clone, PartialEq)]
pub struct Deadlines {
    /// Next timer tick (`App::tick`).
    pub tick: f64,
    /// Next animation frame (`App::next_frame`); ignored while unmapped.
    ///
    /// Invariant: every `App` mutation goes through `Hud::apply` (which
    /// pulls this earlier when an animation starts) or is followed by
    /// `Hud::render` (which recomputes it); otherwise a started animation
    /// would not wake the poll loop until the next tick.
    pub frame: Option<f64>,
    /// Debounced config save.
    pub save: Option<f64>,
    /// Next `Sound::idle_release`.
    pub idle: f64,
    /// When a rate-limited keep-on-top raise becomes allowed.
    pub raise: Option<f64>,
    /// End of the `accept` back-off pause.
    pub accept: Option<f64>,
    /// When the tray tooltip shows (pointer resting on the icon).
    pub tooltip: Option<f64>,
}

impl Deadlines {
    pub fn new(now: f64) -> Deadlines {
        Deadlines {
            tick: now,
            frame: None,
            save: None,
            idle: now + IDLE_RELEASE,
            raise: None,
            accept: None,
            tooltip: None,
        }
    }

    /// Ticks no later than `t`.
    pub fn tick_by(&mut self, t: f64) {
        self.tick = self.tick.min(t);
    }

    /// Draws a frame no later than `t`.
    pub fn frame_by(&mut self, t: f64) {
        self.frame = Some(self.frame.map_or(t, |d| d.min(t)));
    }

    /// Whether the frame is due at `now` (within 1 ms, so the loop does not
    /// wake up for a sub-millisecond remainder).
    pub fn frame_due(&self, now: f64) -> bool {
        self.frame.is_some_and(|t| now >= t - 0.001)
    }

    /// (Re)starts the save debounce.
    pub fn save_soon(&mut self, now: f64) {
        self.save = Some(now + SAVE_DEBOUNCE);
    }

    pub fn save_due(&self, now: f64) -> bool {
        self.save.is_some_and(|t| now >= t)
    }

    /// Whether the sound idle release is due; schedules the next one if so.
    pub fn idle_due(&mut self, now: f64) -> bool {
        let due = now >= self.idle;
        if due {
            self.idle = now + IDLE_RELEASE;
        }
        due
    }

    /// `accept` failed at `now`: pause the listener.
    pub fn pause_accept(&mut self, now: f64) {
        self.accept = Some(now + ACCEPT_BACKOFF);
    }

    /// Whether to poll the listener at `now` (ends an expired pause).
    pub fn accepting(&mut self, now: f64) -> bool {
        if self.accept.is_some_and(|t| now >= t) {
            self.accept = None;
        }
        self.accept.is_none()
    }

    /// Whether the tooltip delay ran out; clears it if so.
    pub fn tooltip_due(&mut self, now: f64) -> bool {
        let due = self.tooltip.is_some_and(|t| now >= t);
        if due {
            self.tooltip = None;
        }
        due
    }

    /// Poll timeout in ms until the earliest deadline; the frame only
    /// counts while the overlay is `mapped`.
    pub fn timeout_ms(&self, now: f64, mapped: bool) -> u16 {
        poll_timeout_ms(
            now,
            &[
                Some(self.tick),
                self.frame.filter(|_| mapped),
                self.save,
                Some(self.idle),
                self.raise,
                self.accept,
                self.tooltip,
            ],
        )
    }
}

/// Poll timeout in ms until the earliest deadline (seconds, relative to now);
/// `None` deadlines are ignored. Rounded up, never negative, at most 1000.
fn poll_timeout_ms(now: f64, deadlines: &[Option<f64>]) -> u16 {
    let earliest = deadlines.iter().flatten().fold(now + 1.0, |a, &d| a.min(d));
    ((earliest - now) * 1000.0).ceil().clamp(0.0, 1000.0) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltip_deadline_wakes_the_loop_once() {
        let mut d = Deadlines::new(0.0);
        d.tick = 5.0;
        d.idle = 5.0;
        d.tooltip = Some(0.6);
        assert_eq!(d.timeout_ms(0.0, false), 600);
        assert!(!d.tooltip_due(0.5));
        assert!(d.tooltip_due(0.6));
        assert!(!d.tooltip_due(0.7), "fires once");
        assert_eq!(d.timeout_ms(0.7, false), 1000);
    }

    #[test]
    fn poll_timeout_picks_earliest_and_rounds_up() {
        assert_eq!(
            poll_timeout_ms(10.0, &[Some(10.5), None, Some(10.0161)]),
            17
        );
        assert_eq!(poll_timeout_ms(10.0, &[Some(9.0)]), 0);
        assert_eq!(poll_timeout_ms(10.0, &[None]), 1000);
        assert_eq!(poll_timeout_ms(10.0, &[Some(99.0)]), 1000);
    }

    #[test]
    fn timeout_waits_for_the_earliest_deadline() {
        // Binary fractions keep the float arithmetic exact.
        let mut d = Deadlines::new(0.0);
        d.tick = 0.75;
        // Idle release at 1.0 and the tick: the tick wins.
        assert_eq!(d.timeout_ms(0.0, true), 750);
        d.save = Some(0.5);
        assert_eq!(d.timeout_ms(0.0, true), 500);
        d.raise = Some(0.25);
        assert_eq!(d.timeout_ms(0.0, true), 250);
        d.pause_accept(0.125 - ACCEPT_BACKOFF);
        assert_eq!(d.timeout_ms(0.0, true), 125);
        d.frame = Some(0.015625);
        assert_eq!(d.timeout_ms(0.0, true), 16, "rounded up");
        // Hidden: animation frames do not wake the loop.
        assert_eq!(d.timeout_ms(0.0, false), 125);
        // Overdue: no sleep.
        assert_eq!(d.timeout_ms(2.0, true), 0);
    }

    #[test]
    fn tick_and_frame_are_only_pulled_earlier() {
        let mut d = Deadlines::new(0.0);
        d.tick = 1.0;
        d.tick_by(2.0);
        assert_eq!(d.tick, 1.0);
        d.tick_by(0.5);
        assert_eq!(d.tick, 0.5);
        d.frame_by(0.3);
        assert_eq!(d.frame, Some(0.3));
        d.frame_by(0.4);
        assert_eq!(d.frame, Some(0.3));
        assert!(!d.frame_due(0.2));
        assert!(d.frame_due(0.2995), "within a millisecond counts as due");
    }

    #[test]
    fn save_debounce_and_idle_release() {
        let mut d = Deadlines::new(0.0);
        assert!(!d.save_due(100.0));
        d.save_soon(1.0);
        d.save_soon(1.2); // another change restarts the debounce
        assert!(!d.save_due(1.4));
        assert!(d.save_due(1.5));
        assert!(!d.idle_due(0.5));
        assert!(d.idle_due(1.0));
        assert_eq!(d.idle, 2.0);
        assert!(!d.idle_due(1.5));
    }

    #[test]
    fn accept_pauses_for_a_second() {
        let mut d = Deadlines::new(0.0);
        assert!(d.accepting(5.0));
        assert_eq!(d.accept, None);
        d.pause_accept(5.0);
        assert!(!d.accepting(5.0));
        assert!(!d.accepting(5.99));
        assert_eq!(d.accept, Some(6.0));
        assert!(d.accepting(6.0));
        assert_eq!(d.accept, None, "an expired pause is no poll deadline");
    }
}
