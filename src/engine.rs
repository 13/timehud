//! Timer state machine (port of `timer_engine.py`). No GUI dependencies; the
//! overlay calls `tick()` every ~100 ms and renders the returned `TickResult`.

use crate::config::{Config, Mode};

pub type ClockFn = Box<dyn Fn() -> f64>;

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Beep {
    /// Short blip (last-5-seconds countdown).
    pub short: bool,
    /// Double beep (sound_alert_before warning / halfway cue).
    pub double: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Run,
    Pause,
    Warn,
    End,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    None,
    Work,
    Rest,
}

#[derive(Clone, Debug)]
pub struct TickResult {
    pub display: f64,
    pub state: State,
    pub beeps: Vec<Beep>,
    pub finished: bool,
    pub restarted: bool,
    pub phase: Phase,
    pub round: i64,
    pub rounds: i64,
    /// Fraction of phase/countdown remaining; -1 = n/a.
    pub progress: f64,
}

pub struct TimerEngine {
    clock: ClockFn,
    running: bool,
    /// clock() when last started.
    start_mono: f64,
    /// Accumulated seconds (stopwatch).
    elapsed: f64,
    cd_remaining: f64,
    sound_beats: i64,
    sound_alert_before_beats: i64,
    last_short_beep_sec: i64,
    phase: Phase,
    round: i64,
    cycle_beats: i64,
    halfway_mark: i64,
    phase_halfway_done: bool,
}

impl TimerEngine {
    pub fn new(cfg: &Config, clock: ClockFn) -> Self {
        let mut e = Self {
            clock,
            running: false,
            start_mono: 0.0,
            elapsed: 0.0,
            cd_remaining: cfg.countdown_duration as f64,
            sound_beats: 0,
            sound_alert_before_beats: 0,
            last_short_beep_sec: -1,
            phase: Phase::Work,
            round: 1,
            cycle_beats: 0,
            halfway_mark: 0,
            phase_halfway_done: false,
        };
        if cfg.timer_mode == Mode::Interval {
            e.cd_remaining = cfg.interval_work as f64;
        }
        e
    }

    fn now(&self) -> f64 {
        (self.clock)()
    }

    // -- Queries --

    pub fn running(&self) -> bool {
        self.running
    }

    /// Stopwatch: total elapsed seconds (in countdown mode: run time since resume).
    pub fn elapsed(&self) -> f64 {
        if self.running {
            self.elapsed + (self.now() - self.start_mono)
        } else {
            self.elapsed
        }
    }

    /// Countdown: seconds remaining.
    pub fn remaining(&self) -> f64 {
        if self.running {
            self.cd_remaining - (self.now() - self.start_mono)
        } else {
            self.cd_remaining
        }
    }

    /// True when stopped at the initial position (never started or reset).
    pub fn is_idle(&self, cfg: &Config) -> bool {
        if self.running {
            return false;
        }
        match cfg.timer_mode {
            Mode::Stopwatch => self.elapsed == 0.0,
            Mode::Interval => {
                self.phase == Phase::Work
                    && self.round == 1
                    && self.cd_remaining == cfg.interval_work as f64
            }
            Mode::Countdown => self.cd_remaining == cfg.countdown_duration as f64,
        }
    }

    // -- Commands --

    /// Start if stopped, pause if running.
    pub fn toggle(&mut self, cfg: &Config) {
        if self.running {
            if cfg.timer_mode == Mode::Stopwatch {
                self.elapsed += self.now() - self.start_mono;
            } else {
                self.cd_remaining -= self.now() - self.start_mono;
                self.cd_remaining = self.cd_remaining.max(0.0);
            }
            self.running = false;
        } else {
            if cfg.timer_mode == Mode::Countdown && self.cd_remaining <= 0.0 {
                self.cd_remaining = cfg.countdown_duration as f64;
            } else if cfg.timer_mode == Mode::Interval && self.cd_remaining <= 0.0 {
                self.phase = Phase::Work;
                self.round = 1;
                self.cd_remaining = cfg.interval_work as f64;
            }
            self.start_mono = self.now();
            self.running = true;
            let si = cfg.sound_interval;
            if si > 0 {
                self.sound_beats = (self.elapsed() / si as f64) as i64;
                self.sound_alert_before_beats =
                    ((self.elapsed() + cfg.sound_alert_before as f64) / si as f64) as i64;
            } else {
                // 0 = periodic beeps off
                self.sound_beats = 0;
                self.sound_alert_before_beats = 0;
            }
            if Self::cycling(cfg) {
                self.cycle_beats = Self::cycle_boundaries(cfg, self.elapsed());
            }
        }
    }

    pub fn reset(&mut self, cfg: &Config) {
        self.running = false;
        self.elapsed = 0.0;
        self.cd_remaining = cfg.countdown_duration as f64;
        self.sound_beats = 0;
        self.sound_alert_before_beats = 0;
        self.last_short_beep_sec = -1;
        self.phase = Phase::Work;
        self.round = 1;
        self.cycle_beats = 0;
        self.halfway_mark = 0;
        self.phase_halfway_done = false;
        if cfg.timer_mode == Mode::Interval {
            self.cd_remaining = cfg.interval_work as f64;
        }
    }

    pub fn set_mode(&mut self, cfg: &mut Config, mode: Mode) {
        cfg.timer_mode = mode;
        self.reset(cfg);
    }

    /// `cfg.countdown_duration` was changed by `delta` seconds; keep the live
    /// remaining time in sync (running) or reload it (stopped).
    pub fn adjust_countdown(&mut self, cfg: &Config, delta: f64) {
        if self.running {
            self.cd_remaining += delta;
        } else {
            self.cd_remaining = cfg.countdown_duration as f64;
        }
    }

    /// Move to the next interval phase. Returns (beep, session_done).
    fn advance_phase(&mut self, cfg: &Config) -> (Beep, bool) {
        if self.phase == Phase::Work {
            if self.round >= cfg.interval_rounds {
                self.running = false;
                self.cd_remaining = 0.0;
                return (Beep::default(), true); // long finish beep at session end
            }
            if cfg.interval_rest > 0 {
                self.phase = Phase::Rest;
                self.cd_remaining = cfg.interval_rest as f64;
            } else {
                self.round += 1;
                self.cd_remaining = cfg.interval_work as f64;
            }
            self.start_mono = self.now();
            self.last_short_beep_sec = -1;
            self.phase_halfway_done = false;
            return (Beep::default(), false); // every phase end is the long beep
        }
        self.phase = Phase::Work;
        self.round += 1;
        self.cd_remaining = cfg.interval_work as f64;
        self.start_mono = self.now();
        self.last_short_beep_sec = -1;
        self.phase_halfway_done = false;
        (Beep::default(), false)
    }

    // -- Tick --

    /// Stopwatch counting upward through work/rest cycles.
    fn cycling(cfg: &Config) -> bool {
        cfg.timer_mode == Mode::Stopwatch && cfg.stopwatch_work > 0
    }

    /// Phase boundaries passed by `elapsed` (work-ends and rest-ends).
    fn cycle_boundaries(cfg: &Config, elapsed: f64) -> i64 {
        let work = cfg.stopwatch_work as f64;
        let rest = cfg.stopwatch_rest;
        let cycle = work + rest as f64;
        let full = (elapsed / cycle).floor() as i64;
        let pos = elapsed - full as f64 * cycle;
        if rest > 0 {
            full * 2 + i64::from(pos >= work)
        } else {
            full
        }
    }

    /// Remaining fraction of the current work/rest phase of a cycling stopwatch.
    fn cycle_progress(cfg: &Config, display: f64) -> Option<f64> {
        let work = cfg.stopwatch_work as f64;
        let rest = cfg.stopwatch_rest as f64;
        let cycle = work + rest;
        let pos = if cycle > 0.0 { display % cycle } else { 0.0 };
        let in_work = pos < work;
        let phase_dur = if in_work { work } else { rest };
        let phase_remaining = if in_work { work - pos } else { cycle - pos };
        (phase_dur > 0.0).then(|| (phase_remaining / phase_dur).clamp(0.0, 1.0))
    }

    /// Countdown/interval progress for `display` seconds left in `phase`.
    fn timed_progress(cfg: &Config, phase: Phase, display: f64) -> f64 {
        let dur = match cfg.timer_mode {
            Mode::Countdown => cfg.countdown_duration as f64,
            Mode::Interval if phase == Phase::Rest => cfg.interval_rest as f64,
            Mode::Interval => cfg.interval_work as f64,
            Mode::Stopwatch => 0.0,
        };
        if dur > 0.0 {
            (display / dur).clamp(0.0, 1.0)
        } else {
            -1.0
        }
    }

    /// Progress `tick()` would report right now, without changing any state
    /// (lets the UI animate the bar every frame between ticks).
    pub fn progress_now(&self, cfg: &Config) -> f64 {
        match cfg.timer_mode {
            Mode::Stopwatch if Self::cycling(cfg) => {
                Self::cycle_progress(cfg, self.elapsed()).unwrap_or(-1.0)
            }
            Mode::Stopwatch => -1.0,
            Mode::Countdown => Self::timed_progress(cfg, self.phase, self.remaining().max(0.0)),
            Mode::Interval => {
                let remaining = self.remaining();
                // Mirror the phase advance `tick()` would perform (`advance_phase`).
                let (phase, display) = if remaining <= 0.0 && self.running {
                    if self.phase == Phase::Rest {
                        (Phase::Work, cfg.interval_work as f64)
                    } else if self.round >= cfg.interval_rounds {
                        (Phase::Work, 0.0) // session over
                    } else if cfg.interval_rest > 0 {
                        (Phase::Rest, cfg.interval_rest as f64)
                    } else {
                        (Phase::Work, cfg.interval_work as f64)
                    }
                } else {
                    (self.phase, remaining.max(0.0))
                };
                Self::timed_progress(cfg, phase, display)
            }
        }
    }

    pub fn tick(&mut self, cfg: &Config) -> TickResult {
        let mut beeps: Vec<Beep> = Vec::new();
        let mut finished = false;
        let mut restarted = false;
        let mut cyc_phase = Phase::None;
        let mut cyc_round = 0;
        let mut cyc_progress: Option<f64> = None;
        let state;
        let display;

        // Warn window: sound_alert_before seconds before the next interval beep
        let mut warn = false;
        if self.running
            && cfg.timer_mode != Mode::Interval
            && !Self::cycling(cfg)
            && cfg.sound_enabled
            && cfg.sound_alert_before > 0
            && cfg.sound_interval > 0
        {
            let reference = self.elapsed();
            let si = cfg.sound_interval as f64;
            let next_beep = ((reference / si) as i64 + 1) as f64 * si;
            warn = next_beep - cfg.sound_alert_before as f64 <= reference && reference < next_beep;
        }

        match cfg.timer_mode {
            Mode::Stopwatch => {
                display = self.elapsed();
                let mut st = if self.running {
                    State::Run
                } else {
                    State::Pause
                };
                if st == State::Run && warn {
                    st = State::Warn;
                }
                if Self::cycling(cfg) {
                    let work = cfg.stopwatch_work as f64;
                    let rest = cfg.stopwatch_rest as f64;
                    let cycle = work + rest;
                    let pos = if cycle > 0.0 { display % cycle } else { 0.0 };
                    let in_work = pos < work;
                    cyc_phase = if in_work { Phase::Work } else { Phase::Rest };
                    cyc_round = (display / cycle).floor() as i64 + 1;
                    let phase_remaining = if in_work { work - pos } else { cycle - pos };
                    cyc_progress = Self::cycle_progress(cfg, display);
                    if self.running {
                        // Boundary beeps: every phase end is the long beep
                        let marks = Self::cycle_boundaries(cfg, display);
                        if marks > self.cycle_beats {
                            self.cycle_beats = marks;
                            if cfg.phase_beeps {
                                beeps.push(Beep::default());
                            }
                            self.last_short_beep_sec = -1;
                        }
                        // Fast double beep halfway through each work phase
                        if cfg.halfway_beep
                            && in_work
                            && pos >= work / 2.0
                            && self.halfway_mark != self.cycle_beats + 1
                        {
                            self.halfway_mark = self.cycle_beats + 1;
                            beeps.push(Beep {
                                short: false,
                                double: true,
                            });
                        }
                        // Last-5 shorts before each boundary, on the label grid
                        if cfg.alert_last_5_seconds && phase_remaining <= 5.0 {
                            st = State::Warn;
                            let sec = phase_remaining.ceil() as i64;
                            if sec != self.last_short_beep_sec && (1..=5).contains(&sec) {
                                self.last_short_beep_sec = sec;
                                beeps.push(Beep {
                                    short: true,
                                    double: false,
                                });
                            }
                        }
                    }
                }
                state = st;
            }
            Mode::Interval => {
                let mut remaining = self.remaining();
                let mut disp = remaining.max(0.0);
                if remaining <= 0.0 {
                    if self.running {
                        let (transition_beep, done) = self.advance_phase(cfg);
                        if cfg.phase_beeps {
                            beeps.push(transition_beep);
                        }
                        if done {
                            finished = true;
                            state = State::End;
                        } else {
                            remaining = self.remaining();
                            disp = remaining.max(0.0);
                            state = State::Run;
                        }
                    } else {
                        state = State::End; // finished session parked at 00:00
                    }
                } else if self.running && remaining <= 5.0 && cfg.alert_last_5_seconds {
                    // Ceil matches the countdown label (which ceils), so the first
                    // short lands exactly as the display flips to 5.
                    let sec_display = remaining.ceil() as i64;
                    state = State::Warn;
                    if sec_display != self.last_short_beep_sec && (1..=5).contains(&sec_display) {
                        self.last_short_beep_sec = sec_display;
                        beeps.push(Beep {
                            short: true,
                            double: false,
                        });
                    }
                } else {
                    state = if self.running {
                        State::Run
                    } else {
                        State::Pause
                    };
                }
                // Fast double beep halfway through each work phase
                if self.running
                    && cfg.halfway_beep
                    && self.phase == Phase::Work
                    && 0.0 < remaining
                    && remaining <= cfg.interval_work as f64 / 2.0
                    && !self.phase_halfway_done
                {
                    self.phase_halfway_done = true;
                    beeps.push(Beep {
                        short: false,
                        double: true,
                    });
                }
                display = disp;
            }
            Mode::Countdown => {
                let remaining = self.remaining();
                display = remaining.max(0.0);
                let sec_display = display.ceil() as i64; // matches the ceiled label
                if remaining <= 0.0 {
                    state = State::End;
                    if self.running {
                        finished = true;
                        beeps.push(Beep::default()); // long beep exactly when time runs out
                        if cfg.auto_restart_countdown {
                            self.cd_remaining = cfg.countdown_duration as f64;
                            self.start_mono = self.now();
                            self.sound_beats = 0;
                            self.last_short_beep_sec = -1;
                            restarted = true;
                        } else {
                            self.running = false;
                        }
                    }
                } else if self.running && remaining <= 5.0 && cfg.alert_last_5_seconds {
                    state = if sec_display == 1 {
                        State::End
                    } else {
                        State::Warn
                    };
                    if sec_display != self.last_short_beep_sec && (1..=5).contains(&sec_display) {
                        self.last_short_beep_sec = sec_display;
                        beeps.push(Beep {
                            short: true,
                            double: false,
                        });
                    }
                } else if self.running {
                    state = if warn && remaining > 5.0 {
                        State::Warn
                    } else {
                        State::Run
                    };
                } else {
                    state = State::Pause;
                }
            }
        }

        // Periodic interval beeps (not while a cycle structure provides beeps)
        if self.running
            && cfg.timer_mode != Mode::Interval
            && !Self::cycling(cfg)
            && cfg.sound_enabled
            && cfg.sound_interval > 0
        {
            let reference = self.elapsed();
            let si = cfg.sound_interval as f64;
            if cfg.sound_alert_before > 0 {
                let target =
                    (self.sound_alert_before_beats + 1) as f64 * si - cfg.sound_alert_before as f64;
                if target > 0.0 && reference >= target {
                    self.sound_alert_before_beats += 1;
                    beeps.push(Beep {
                        short: false,
                        double: true,
                    });
                }
            }
            let beats = (reference / si) as i64;
            if beats > self.sound_beats {
                self.sound_beats = beats;
                beeps.push(Beep::default());
            }
        }

        let progress =
            cyc_progress.unwrap_or_else(|| Self::timed_progress(cfg, self.phase, display));

        let in_interval = cfg.timer_mode == Mode::Interval;
        TickResult {
            display,
            state,
            beeps,
            finished,
            restarted,
            phase: if in_interval { self.phase } else { cyc_phase },
            round: if in_interval { self.round } else { cyc_round },
            rounds: if in_interval { cfg.interval_rounds } else { 0 },
            progress,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[derive(Clone)]
    struct FakeClock(Rc<Cell<f64>>);

    impl FakeClock {
        fn new() -> Self {
            FakeClock(Rc::new(Cell::new(1000.0)))
        }
        fn advance(&self, secs: f64) {
            self.0.set(self.0.get() + secs);
        }
        fn clock_fn(&self) -> ClockFn {
            let c = self.0.clone();
            Box::new(move || c.get())
        }
    }

    fn approx(a: f64, b: f64) {
        assert!((a - b).abs() < 1e-6, "{a} != {b}");
    }

    fn approx_abs(a: f64, b: f64, tol: f64) {
        assert!((a - b).abs() <= tol, "{a} != {b} (tol {tol})");
    }

    fn is_long(b: &Beep) -> bool {
        !b.short && !b.double
    }

    fn longs(beeps: &[Beep]) -> usize {
        beeps.iter().filter(|b| is_long(b)).count()
    }

    fn shorts(beeps: &[Beep]) -> usize {
        beeps.iter().filter(|b| b.short).count()
    }

    fn doubles(beeps: &[Beep]) -> usize {
        beeps.iter().filter(|b| b.double).count()
    }

    /// Advance in `step` increments, ticking like the 100 ms UI loop.
    fn collect_beeps(
        engine: &mut TimerEngine,
        cfg: &Config,
        clock: &FakeClock,
        seconds: f64,
    ) -> Vec<Beep> {
        let step = 0.1;
        let mut out = Vec::new();
        let n = (seconds / step) as i64;
        for _ in 0..n {
            clock.advance(step);
            out.extend(engine.tick(cfg).beeps);
        }
        out
    }

    fn make(cfg: &Config) -> (TimerEngine, FakeClock) {
        let clock = FakeClock::new();
        let engine = TimerEngine::new(cfg, clock.clock_fn());
        (engine, clock)
    }

    mod test_stopwatch {
        use super::*;

        #[test]
        fn starts_at_zero_paused() {
            let cfg = Config::default();
            let (e, _c) = make(&cfg);
            assert!(!e.running());
            assert_eq!(e.elapsed(), 0.0);
        }

        #[test]
        fn accumulates_while_running() {
            let cfg = Config::default();
            let (mut e, c) = make(&cfg);
            e.toggle(&cfg);
            c.advance(12.5);
            approx(e.elapsed(), 12.5);
        }

        #[test]
        fn pause_freezes_and_resume_continues() {
            let cfg = Config::default();
            let (mut e, c) = make(&cfg);
            e.toggle(&cfg);
            c.advance(10.0);
            e.toggle(&cfg); // pause
            c.advance(99.0); // time passes while paused
            approx(e.elapsed(), 10.0);
            e.toggle(&cfg); // resume
            c.advance(5.0);
            approx(e.elapsed(), 15.0);
        }

        #[test]
        fn reset_zeroes() {
            let cfg = Config::default();
            let (mut e, c) = make(&cfg);
            e.toggle(&cfg);
            c.advance(30.0);
            e.reset(&cfg);
            assert!(!e.running());
            assert_eq!(e.elapsed(), 0.0);
        }

        #[test]
        fn tick_state_run_and_pause() {
            let cfg = Config::default();
            let (mut e, c) = make(&cfg);
            assert_eq!(e.tick(&cfg).state, State::Pause);
            e.toggle(&cfg);
            c.advance(3.0);
            let r = e.tick(&cfg);
            assert_eq!(r.state, State::Run);
            approx(r.display, 3.0);
        }
    }

    mod test_countdown {
        use super::*;

        fn setup() -> (Config, TimerEngine, FakeClock) {
            let cfg = Config {
                timer_mode: Mode::Countdown,
                countdown_duration: 60,
                ..Config::default()
            };
            let (e, c) = make(&cfg);
            (cfg, e, c)
        }

        #[test]
        fn counts_down() {
            let (cfg, mut e, c) = setup();
            e.toggle(&cfg);
            c.advance(25.0);
            approx(e.remaining(), 35.0);
            approx(e.tick(&cfg).display, 35.0);
        }

        #[test]
        fn pause_resume() {
            let (cfg, mut e, c) = setup();
            e.toggle(&cfg);
            c.advance(20.0);
            e.toggle(&cfg);
            c.advance(500.0);
            approx(e.remaining(), 40.0);
        }

        #[test]
        fn finish_stops_engine() {
            let (cfg, mut e, c) = setup();
            e.toggle(&cfg);
            c.advance(61.0);
            let r = e.tick(&cfg);
            assert_eq!(r.state, State::End);
            assert!(r.finished);
            assert_eq!(r.display, 0.0);
            assert!(!e.running());
            // subsequent ticks stay ended but don't re-report finished
            assert!(!e.tick(&cfg).finished);
        }

        #[test]
        fn auto_restart() {
            let (mut cfg, mut e, c) = setup();
            cfg.auto_restart_countdown = true;
            e.toggle(&cfg);
            c.advance(61.0);
            let r = e.tick(&cfg);
            assert!(r.restarted);
            assert!(e.running());
            approx(e.remaining(), 60.0);
        }

        #[test]
        fn start_after_finish_reloads_duration() {
            let (cfg, mut e, c) = setup();
            e.toggle(&cfg);
            c.advance(61.0);
            e.tick(&cfg);
            e.toggle(&cfg); // restart from full duration
            approx(e.remaining(), 60.0);
        }

        #[test]
        fn adjust_countdown_while_running() {
            let (mut cfg, mut e, c) = setup();
            e.toggle(&cfg);
            c.advance(10.0);
            cfg.countdown_duration += 60;
            e.adjust_countdown(&cfg, 60.0);
            approx(e.remaining(), 110.0);
        }

        #[test]
        fn adjust_countdown_while_stopped_reloads() {
            let (mut cfg, mut e, _c) = setup();
            cfg.countdown_duration = 90;
            e.adjust_countdown(&cfg, 30.0);
            approx(e.remaining(), 90.0);
        }
    }

    mod test_mode_switch {
        use super::*;

        #[test]
        fn set_mode_resets() {
            let mut cfg = Config::default();
            let (mut e, c) = make(&cfg);
            e.toggle(&cfg);
            c.advance(42.0);
            e.set_mode(&mut cfg, Mode::Countdown);
            assert_eq!(cfg.timer_mode, Mode::Countdown);
            assert!(!e.running());
            approx(e.remaining(), cfg.countdown_duration as f64);
        }
    }

    mod test_interval_beeps {
        use super::*;

        fn setup() -> (Config, TimerEngine, FakeClock) {
            let cfg = Config {
                timer_mode: Mode::Stopwatch,
                sound_enabled: true,
                sound_interval: 60,
                sound_alert_before: 0,
                ..Config::default()
            };
            let (e, c) = make(&cfg);
            (cfg, e, c)
        }

        #[test]
        fn main_beep_every_interval() {
            let (cfg, mut e, c) = setup();
            e.toggle(&cfg);
            let beeps = collect_beeps(&mut e, &cfg, &c, 125.0);
            assert_eq!(longs(&beeps), 2); // at 60 s and 120 s
        }

        #[test]
        fn no_beeps_when_sound_disabled() {
            let (mut cfg, mut e, c) = setup();
            cfg.sound_enabled = false;
            e.toggle(&cfg);
            assert_eq!(collect_beeps(&mut e, &cfg, &c, 125.0), vec![]);
        }

        #[test]
        fn no_beeps_while_paused() {
            let (cfg, mut e, c) = setup();
            assert_eq!(collect_beeps(&mut e, &cfg, &c, 125.0), vec![]);
        }

        #[test]
        fn alert_before_double_beep() {
            let (mut cfg, mut e, c) = setup();
            cfg.sound_alert_before = 5;
            e.reset(&cfg);
            e.toggle(&cfg);
            let beeps = collect_beeps(&mut e, &cfg, &c, 59.0);
            assert_eq!(doubles(&beeps), 1); // at 55 s (60 - 5)
            let beeps = collect_beeps(&mut e, &cfg, &c, 3.0); // through 62 s
            assert_eq!(longs(&beeps), 1); // main beep still at 60 s
        }

        #[test]
        fn pause_resume_does_not_replay_past_beeps() {
            let (cfg, mut e, c) = setup();
            e.toggle(&cfg);
            collect_beeps(&mut e, &cfg, &c, 70.0); // beep at 60 consumed
            e.toggle(&cfg); // pause at 70 s
            e.toggle(&cfg); // resume
            let beeps = collect_beeps(&mut e, &cfg, &c, 5.0);
            assert_eq!(beeps, vec![]); // nothing until 120 s
        }
    }

    mod test_last_five_seconds {
        use super::*;

        fn setup() -> (Config, TimerEngine, FakeClock) {
            let cfg = Config {
                timer_mode: Mode::Countdown,
                countdown_duration: 30,
                sound_enabled: true,
                sound_interval: 6000, // keep interval beeps out of the way
                alert_last_5_seconds: true,
                ..Config::default()
            };
            let (e, c) = make(&cfg);
            (cfg, e, c)
        }

        #[test]
        fn short_beeps_then_long_at_true_zero() {
            let (cfg, mut e, c) = setup();
            e.toggle(&cfg);
            let beeps = collect_beeps(&mut e, &cfg, &c, 29.95); // label shows 00:00, not done
            assert_eq!(shorts(&beeps), 5); // displayed 5,4,3,2,1
            assert_eq!(longs(&beeps), 0); // nothing during the displayed-zero second
            let beeps = collect_beeps(&mut e, &cfg, &c, 0.2); // cross true zero (30 s)
            assert_eq!(longs(&beeps), 1); // long beep exactly when time runs out
        }

        #[test]
        fn first_short_fires_when_label_shows_five() {
            let (cfg, mut e, c) = setup();
            e.toggle(&cfg);
            c.advance(25.05); // remaining 4.95 -> ceiled label "00:05"
            let beeps = e.tick(&cfg).beeps;
            assert!(
                beeps.iter().any(|b| b.short),
                "short beep must start at displayed 5"
            );
        }

        #[test]
        fn no_short_while_label_shows_six() {
            let (cfg, mut e, c) = setup();
            e.toggle(&cfg);
            c.advance(24.4); // remaining 5.6 -> ceiled label "00:06"
            assert_eq!(e.tick(&cfg).beeps, vec![]);
        }

        #[test]
        fn finish_beep_without_last5_flag() {
            let (mut cfg, mut e, c) = setup();
            cfg.alert_last_5_seconds = false;
            e.reset(&cfg);
            e.toggle(&cfg);
            let beeps = collect_beeps(&mut e, &cfg, &c, 30.2);
            assert_eq!(shorts(&beeps), 0);
            assert_eq!(longs(&beeps), 1); // finish beep independent of last-5 flag
        }

        #[test]
        fn warn_then_end_state() {
            let (cfg, mut e, c) = setup();
            e.toggle(&cfg);
            c.advance(26.5); // remaining 3.5 -> displayed 4
            assert_eq!(e.tick(&cfg).state, State::Warn);
            c.advance(2.6); // remaining 0.9 -> displayed 1
            assert_eq!(e.tick(&cfg).state, State::End);
        }

        #[test]
        fn disabled_flag_suppresses() {
            let (mut cfg, mut e, c) = setup();
            cfg.alert_last_5_seconds = false;
            e.toggle(&cfg);
            let beeps = collect_beeps(&mut e, &cfg, &c, 29.5);
            assert_eq!(shorts(&beeps), 0);
        }
    }

    mod test_is_idle {
        use super::*;

        #[test]
        fn idle_at_reset() {
            let cfg = Config::default();
            let (e, _c) = make(&cfg);
            assert!(e.is_idle(&cfg));
        }

        #[test]
        fn not_idle_while_running() {
            let cfg = Config::default();
            let (mut e, c) = make(&cfg);
            e.toggle(&cfg);
            c.advance(1.0);
            assert!(!e.is_idle(&cfg));
        }

        #[test]
        fn not_idle_when_paused_midway() {
            let cfg = Config::default();
            let (mut e, c) = make(&cfg);
            e.toggle(&cfg);
            c.advance(5.0);
            e.toggle(&cfg);
            assert!(!e.is_idle(&cfg));
        }

        #[test]
        fn idle_again_after_reset() {
            let cfg = Config::default();
            let (mut e, c) = make(&cfg);
            e.toggle(&cfg);
            c.advance(5.0);
            e.reset(&cfg);
            assert!(e.is_idle(&cfg));
        }

        #[test]
        fn idle_countdown_at_full_duration() {
            let cfg = Config {
                timer_mode: Mode::Countdown,
                countdown_duration: 60,
                ..Config::default()
            };
            let (mut e, c) = make(&cfg);
            assert!(e.is_idle(&cfg));
            e.toggle(&cfg);
            c.advance(3.0);
            e.toggle(&cfg);
            assert!(!e.is_idle(&cfg));
        }
    }

    mod test_interval {
        use super::*;

        fn setup() -> (Config, TimerEngine, FakeClock) {
            let cfg = Config {
                timer_mode: Mode::Interval,
                interval_work: 40,
                interval_rest: 20,
                interval_rounds: 3,
                sound_enabled: true,
                sound_interval: 60, // must NOT fire in interval mode
                alert_last_5_seconds: false,
                ..Config::default()
            };
            let (e, c) = make(&cfg);
            (cfg, e, c)
        }

        #[test]
        fn idle_start() {
            let (cfg, mut e, _c) = setup();
            assert!(e.is_idle(&cfg));
            let r = e.tick(&cfg);
            assert!(r.phase == Phase::Work && r.round == 1 && r.rounds == 3);
            approx(r.display, 40.0);
        }

        #[test]
        fn work_to_rest_transition() {
            let (cfg, mut e, c) = setup();
            e.toggle(&cfg);
            c.advance(40.05);
            let r = e.tick(&cfg);
            assert!(r.phase == Phase::Rest && r.round == 1);
            assert!(r.beeps.iter().any(is_long), "phase end = long beep");
            approx_abs(r.display, 20.0, 0.1);
        }

        #[test]
        fn rest_to_work_advances_round() {
            let (cfg, mut e, c) = setup();
            e.toggle(&cfg);
            c.advance(40.05);
            e.tick(&cfg);
            c.advance(20.05);
            let r = e.tick(&cfg);
            assert!(r.phase == Phase::Work && r.round == 2);
            assert!(r.beeps.iter().any(is_long));
        }

        #[test]
        fn session_finish_skips_last_rest() {
            let (cfg, mut e, c) = setup();
            e.toggle(&cfg);
            // 3 rounds: W R W R W -> end (no trailing rest)
            for _ in 0..2 {
                c.advance(40.05);
                e.tick(&cfg); // work ends
                c.advance(20.05);
                e.tick(&cfg); // rest ends
            }
            c.advance(40.05);
            let r = e.tick(&cfg);
            assert!(r.finished && r.state == State::End);
            assert!(!e.running());
            // parked at zero afterwards
            let r2 = e.tick(&cfg);
            assert!(r2.state == State::End && r2.display == 0.0);
        }

        #[test]
        fn zero_rest_back_to_back() {
            let (mut cfg, mut e, c) = setup();
            cfg.interval_rest = 0;
            e.reset(&cfg);
            e.toggle(&cfg);
            c.advance(40.05);
            let r = e.tick(&cfg);
            assert!(r.phase == Phase::Work && r.round == 2);
            assert!(r.beeps.iter().any(is_long));
        }

        #[test]
        fn no_periodic_beeps_in_interval() {
            let (cfg, mut e, c) = setup();
            e.toggle(&cfg);
            let beeps = collect_beeps(&mut e, &cfg, &c, 39.0); // sound_interval=60 stays silent
            assert_eq!(beeps, vec![]);
        }

        #[test]
        fn last5_shorts_no_long() {
            let (mut cfg, mut e, c) = setup();
            cfg.alert_last_5_seconds = true;
            e.reset(&cfg);
            e.toggle(&cfg);
            let beeps = collect_beeps(&mut e, &cfg, &c, 39.5);
            assert_eq!(shorts(&beeps), 5); // displayed 5,4,3,2,1
            assert_eq!(longs(&beeps), 0); // transition beep covers the zero mark
        }

        #[test]
        fn pause_resume_mid_phase() {
            let (cfg, mut e, c) = setup();
            e.toggle(&cfg);
            c.advance(10.0);
            e.toggle(&cfg);
            c.advance(500.0);
            approx(e.remaining(), 30.0);
            assert!(!e.is_idle(&cfg));
        }

        #[test]
        fn interval_halfway_double_in_work() {
            let (mut cfg, mut e, c) = setup();
            cfg.halfway_beep = true;
            e.reset(&cfg);
            e.toggle(&cfg);
            let beeps = collect_beeps(&mut e, &cfg, &c, 39.0); // work phase (40 s, half = 20)
            assert_eq!(doubles(&beeps), 1); // halfway at 20 s
        }

        #[test]
        fn interval_transitions_all_long() {
            let (cfg, mut e, c) = setup();
            e.toggle(&cfg);
            let beeps = collect_beeps(&mut e, &cfg, &c, 61.0); // work->rest at 40, rest->work at 60
            assert_eq!(longs(&beeps), 2);
            assert_eq!(doubles(&beeps), 0);
        }

        #[test]
        fn interval_phase_beeps_toggle_off() {
            let (mut cfg, mut e, c) = setup();
            cfg.phase_beeps = false;
            e.reset(&cfg);
            e.toggle(&cfg);
            let beeps = collect_beeps(&mut e, &cfg, &c, 61.0); // crosses work->rest->work
            assert_eq!(doubles(&beeps), 0);
            assert_eq!(longs(&beeps), 0);
        }

        #[test]
        fn restart_after_finish() {
            let (cfg, mut e, c) = setup();
            e.toggle(&cfg);
            for _ in 0..2 {
                c.advance(40.05);
                e.tick(&cfg);
                c.advance(20.05);
                e.tick(&cfg);
            }
            c.advance(40.05);
            e.tick(&cfg); // finished
            e.toggle(&cfg); // start again from round 1
            let r = e.tick(&cfg);
            assert!(e.running());
            assert!(r.phase == Phase::Work && r.round == 1);
            approx_abs(r.display, 40.0, 0.1);
        }
    }

    mod test_zero_sound_interval {
        use super::*;

        #[test]
        fn stopwatch_no_periodic_no_crash() {
            let cfg = Config {
                timer_mode: Mode::Stopwatch,
                sound_enabled: true,
                sound_interval: 0,
                sound_alert_before: 5,
                ..Config::default()
            };
            let (mut e, c) = make(&cfg);
            e.toggle(&cfg);
            assert_eq!(collect_beeps(&mut e, &cfg, &c, 120.0), vec![]);
        }

        #[test]
        fn countdown_finish_beep_still_fires() {
            let cfg = Config {
                timer_mode: Mode::Countdown,
                countdown_duration: 10,
                sound_enabled: true,
                sound_interval: 0,
                ..Config::default()
            };
            let (mut e, c) = make(&cfg);
            e.toggle(&cfg);
            let beeps = collect_beeps(&mut e, &cfg, &c, 10.2);
            assert_eq!(longs(&beeps), 1);
        }
    }

    mod test_cycling_stopwatch {
        use super::*;

        fn setup() -> (Config, TimerEngine, FakeClock) {
            let cfg = Config {
                timer_mode: Mode::Stopwatch,
                stopwatch_work: 45,
                stopwatch_rest: 15,
                sound_enabled: true,
                sound_interval: 60, // must NOT fire while cycling
                alert_last_5_seconds: false,
                ..Config::default()
            };
            let (e, c) = make(&cfg);
            (cfg, e, c)
        }

        #[test]
        fn counts_upward_with_phase_info() {
            let (cfg, mut e, c) = setup();
            e.toggle(&cfg);
            c.advance(10.0);
            let r = e.tick(&cfg);
            approx(r.display, 10.0); // count-up display
            assert!(r.phase == Phase::Work && r.round == 1);
            assert_eq!(r.rounds, 0); // endless
            c.advance(40.0); // elapsed 50 -> rest of round 1
            let r = e.tick(&cfg);
            approx(r.display, 50.0);
            assert!(r.phase == Phase::Rest && r.round == 1);
        }

        #[test]
        fn round_increments_each_cycle() {
            let (cfg, mut e, c) = setup();
            e.toggle(&cfg);
            c.advance(61.0); // into round 2 work
            let r = e.tick(&cfg);
            assert!(r.phase == Phase::Work && r.round == 2);
        }

        #[test]
        fn boundary_beeps_all_long() {
            let (cfg, mut e, c) = setup();
            e.toggle(&cfg);
            let beeps = collect_beeps(&mut e, &cfg, &c, 121.0);
            assert_eq!(longs(&beeps), 4); // 45, 60, 105, 120 s
            assert_eq!(doubles(&beeps), 0); // double is halfway-only
        }

        #[test]
        fn zero_rest_long_beeps_only() {
            let (mut cfg, mut e, c) = setup();
            cfg.stopwatch_rest = 0;
            e.reset(&cfg);
            e.toggle(&cfg);
            let beeps = collect_beeps(&mut e, &cfg, &c, 100.0);
            assert_eq!(doubles(&beeps), 0);
            assert_eq!(longs(&beeps), 2); // work ends at 45s and 90s
        }

        #[test]
        fn no_periodic_interval_beeps_while_cycling() {
            let (cfg, mut e, c) = setup();
            e.toggle(&cfg);
            let beeps = collect_beeps(&mut e, &cfg, &c, 44.0); // before first boundary
            assert_eq!(beeps, vec![]); // sound_interval=60 stays quiet
        }

        #[test]
        fn last5_shorts_before_each_boundary() {
            let (mut cfg, mut e, c) = setup();
            cfg.alert_last_5_seconds = true;
            e.reset(&cfg);
            e.toggle(&cfg);
            let beeps = collect_beeps(&mut e, &cfg, &c, 44.5); // work phase, 0.5s left
            assert_eq!(shorts(&beeps), 5); // phase-remaining 5..1
        }

        #[test]
        fn progress_tracks_phase_remaining() {
            let (cfg, mut e, c) = setup();
            e.toggle(&cfg);
            c.advance(40.0); // 5s left in 45s work
            let r = e.tick(&cfg);
            approx_abs(r.progress, 5.0 / 45.0, 0.01);
            c.advance(10.0); // 10s left in 15s rest
            let r = e.tick(&cfg);
            approx_abs(r.progress, 10.0 / 15.0, 0.01);
        }

        #[test]
        fn pause_resume_does_not_replay_boundaries() {
            let (cfg, mut e, c) = setup();
            e.toggle(&cfg);
            collect_beeps(&mut e, &cfg, &c, 50.0); // long at 45 consumed
            e.toggle(&cfg); // pause in rest
            e.toggle(&cfg); // resume
            let beeps = collect_beeps(&mut e, &cfg, &c, 5.0); // elapsed 55, next boundary 60
            assert_eq!(beeps, vec![]);
        }

        #[test]
        fn halfway_double_in_each_work_phase() {
            let (mut cfg, mut e, c) = setup();
            cfg.halfway_beep = true;
            e.reset(&cfg);
            e.toggle(&cfg);
            let beeps = collect_beeps(&mut e, &cfg, &c, 44.0); // work phase (45 s, half = 22.5)
            assert_eq!(doubles(&beeps), 1); // halfway at 22.5 s
            let beeps = collect_beeps(&mut e, &cfg, &c, 15.0); // rest: no halfway cue
            assert_eq!(doubles(&beeps), 0);
            let beeps = collect_beeps(&mut e, &cfg, &c, 25.0); // round 2 work halfway at 82.5
            assert_eq!(doubles(&beeps), 1);
        }

        #[test]
        fn halfway_not_replayed_after_pause() {
            let (mut cfg, mut e, c) = setup();
            cfg.halfway_beep = true;
            e.reset(&cfg);
            e.toggle(&cfg);
            collect_beeps(&mut e, &cfg, &c, 25.0); // halfway at 22.5 consumed
            e.toggle(&cfg);
            e.toggle(&cfg);
            let beeps = collect_beeps(&mut e, &cfg, &c, 3.0);
            assert_eq!(doubles(&beeps), 0);
        }

        #[test]
        fn halfway_off_by_default() {
            let (cfg, mut e, c) = setup();
            e.toggle(&cfg);
            let beeps = collect_beeps(&mut e, &cfg, &c, 44.0);
            assert_eq!(doubles(&beeps), 0);
        }

        #[test]
        fn rest_phase_keeps_run_state_outside_last5() {
            let (mut cfg, mut e, c) = setup();
            cfg.halfway_beep = true;
            cfg.alert_last_5_seconds = true;
            e.reset(&cfg);
            e.toggle(&cfg);
            c.advance(47.0); // rest, 13 s remaining
            let r = e.tick(&cfg);
            assert!(r.phase == Phase::Rest && r.state == State::Run); // blue rest, no warn tint
        }

        #[test]
        fn phase_beeps_toggle_off() {
            let (mut cfg, mut e, c) = setup();
            cfg.phase_beeps = false;
            e.reset(&cfg);
            e.toggle(&cfg);
            let beeps = collect_beeps(&mut e, &cfg, &c, 121.0); // two full cycles
            assert_eq!(doubles(&beeps) + longs(&beeps), 0);
        }

        #[test]
        fn plain_stopwatch_unchanged() {
            let cfg = Config {
                timer_mode: Mode::Stopwatch,
                stopwatch_work: 0,
                ..Config::default()
            };
            let (mut e, c) = make(&cfg);
            e.toggle(&cfg);
            c.advance(10.0);
            let r = e.tick(&cfg);
            assert!(r.phase == Phase::None && r.round == 0);
            assert_eq!(r.progress, -1.0);
        }
    }

    mod test_progress {
        use super::*;

        #[test]
        fn stopwatch_has_no_progress() {
            let cfg = Config::default();
            let (mut e, c) = make(&cfg);
            e.toggle(&cfg);
            c.advance(5.0);
            assert_eq!(e.tick(&cfg).progress, -1.0);
        }

        #[test]
        fn countdown_progress() {
            let cfg = Config {
                timer_mode: Mode::Countdown,
                countdown_duration: 100,
                ..Config::default()
            };
            let (mut e, c) = make(&cfg);
            e.toggle(&cfg);
            c.advance(25.0);
            approx(e.tick(&cfg).progress, 0.75);
        }

        #[test]
        fn interval_progress_per_phase() {
            let cfg = Config {
                timer_mode: Mode::Interval,
                interval_work: 40,
                interval_rest: 20,
                interval_rounds: 2,
                ..Config::default()
            };
            let (mut e, c) = make(&cfg);
            e.toggle(&cfg);
            c.advance(30.0);
            approx(e.tick(&cfg).progress, 0.25); // 10/40 left
            c.advance(10.05);
            e.tick(&cfg); // into rest
            c.advance(5.0);
            approx_abs(e.tick(&cfg).progress, 15.0 / 20.0, 0.01);
        }
    }

    mod test_progress_now {
        use super::*;

        /// `progress_now` must predict `tick().progress` at the same instant.
        fn check(cfg: &Config, steps: usize, step: f64, toggles: &[usize]) {
            let (mut e, c) = make(cfg);
            e.toggle(cfg);
            for i in 0..steps {
                if toggles.contains(&i) {
                    e.toggle(cfg);
                }
                c.advance(step);
                let predicted = e.progress_now(cfg);
                assert_eq!(predicted, e.progress_now(cfg), "query changed state");
                let actual = e.tick(cfg).progress;
                assert!(
                    (predicted - actual).abs() < 1e-12,
                    "step {i}: progress_now {predicted} != tick {actual}"
                );
            }
        }

        #[test]
        fn matches_tick_countdown() {
            let cfg = Config {
                timer_mode: Mode::Countdown,
                countdown_duration: 7,
                ..Config::default()
            };
            check(&cfg, 100, 0.1, &[30, 40]);
            let auto = Config {
                auto_restart_countdown: true,
                ..cfg
            };
            check(&auto, 200, 0.1, &[]);
        }

        #[test]
        fn matches_tick_interval_across_phases_and_end() {
            let cfg = Config {
                timer_mode: Mode::Interval,
                interval_work: 3,
                interval_rest: 2,
                interval_rounds: 2,
                ..Config::default()
            };
            check(&cfg, 150, 0.1, &[12, 20]);
            let no_rest = Config {
                interval_rest: 0,
                ..cfg
            };
            check(&no_rest, 100, 0.1, &[]);
        }

        #[test]
        fn matches_tick_stopwatch() {
            check(&Config::default(), 30, 0.1, &[10]);
            let cyc = Config {
                stopwatch_work: 3,
                stopwatch_rest: 2,
                ..Config::default()
            };
            check(&cyc, 150, 0.1, &[7, 9]);
        }
    }
}
