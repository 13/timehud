"""
timer_engine.py – Timer state machine for TimeHUD.

Pure Python, no Qt. The overlay calls tick() every ~100 ms and renders
the returned TickResult; commands (toggle/reset/…) come from UI events.
The injectable `clock` (defaults to time.monotonic) makes tests deterministic.
"""

import math
import time
from dataclasses import dataclass, field


@dataclass
class Beep:
    short: bool = False    # short blip (last-5-seconds countdown)
    double: bool = False   # double beep (sound_alert_before warning)


@dataclass
class TickResult:
    display: float                      # seconds to render
    state: str                          # "run" | "pause" | "warn" | "end"
    beeps: list = field(default_factory=list)
    finished: bool = False              # countdown hit zero on this tick
    restarted: bool = False             # auto_restart_countdown kicked in
    phase: str = ""          # "work" | "rest" in interval mode, else ""
    round: int = 0           # 1-based current round (interval mode)
    rounds: int = 0          # configured rounds (interval mode)
    progress: float = -1.0   # fraction of phase/countdown remaining; -1 = n/a


def fmt_seconds(secs: float) -> str:
    """Format seconds → MM:SS, or HH:MM:SS from one hour up."""
    s = max(0, int(secs))
    h, rem = divmod(s, 3600)
    m, sec = divmod(rem, 60)
    if h:
        return f"{h:02d}:{m:02d}:{sec:02d}"
    return f"{m:02d}:{sec:02d}"


class TimerEngine:
    def __init__(self, config, clock=time.monotonic) -> None:
        self.config = config
        self._clock = clock
        self.running = False
        self._start_mono = 0.0    # clock() when last started
        self._elapsed = 0.0       # accumulated seconds (stopwatch)
        self._cd_remaining = float(config.countdown_duration)
        self._sound_beats = 0
        self._sound_alert_before_beats = 0
        self._last_short_beep_sec = -1
        self._phase = "work"
        self._round = 1
        self._cycle_beats = 0
        if config.timer_mode == "interval":
            self._cd_remaining = float(config.interval_work)

    # ── Queries ────────────────────────────────────────────────────────
    def elapsed(self) -> float:
        """Stopwatch: total elapsed seconds (in countdown mode: run time since resume)."""
        if self.running:
            return self._elapsed + (self._clock() - self._start_mono)
        return self._elapsed

    def remaining(self) -> float:
        """Countdown: seconds remaining."""
        if self.running:
            return self._cd_remaining - (self._clock() - self._start_mono)
        return self._cd_remaining

    def is_idle(self) -> bool:
        """True when stopped at the initial position (never started or reset)."""
        if self.running:
            return False
        if self.config.timer_mode == "stopwatch":
            return self._elapsed == 0.0
        if self.config.timer_mode == "interval":
            return (
                self._phase == "work"
                and self._round == 1
                and self._cd_remaining == float(self.config.interval_work)
            )
        return self._cd_remaining == float(self.config.countdown_duration)

    # ── Commands ───────────────────────────────────────────────────────
    def toggle(self) -> None:
        """Start if stopped, pause if running."""
        if self.running:
            if self.config.timer_mode == "stopwatch":
                self._elapsed += self._clock() - self._start_mono
            else:
                self._cd_remaining -= self._clock() - self._start_mono
                self._cd_remaining = max(0.0, self._cd_remaining)
            self.running = False
        else:
            if self.config.timer_mode == "countdown" and self._cd_remaining <= 0:
                self._cd_remaining = float(self.config.countdown_duration)
            elif self.config.timer_mode == "interval" and self._cd_remaining <= 0:
                self._phase = "work"
                self._round = 1
                self._cd_remaining = float(self.config.interval_work)
            self._start_mono = self._clock()
            self.running = True
            si = self.config.sound_interval
            if si > 0:
                self._sound_beats = int(self.elapsed() / si)
                self._sound_alert_before_beats = int(
                    (self.elapsed() + self.config.sound_alert_before) / si
                )
            else:   # 0 = periodic beeps off
                self._sound_beats = 0
                self._sound_alert_before_beats = 0
            if self._cycling():
                self._cycle_beats = self._cycle_boundaries(self.elapsed())

    def reset(self) -> None:
        self.running = False
        self._elapsed = 0.0
        self._cd_remaining = float(self.config.countdown_duration)
        self._sound_beats = 0
        self._sound_alert_before_beats = 0
        self._last_short_beep_sec = -1
        self._phase = "work"
        self._round = 1
        self._cycle_beats = 0
        if self.config.timer_mode == "interval":
            self._cd_remaining = float(self.config.interval_work)

    def set_mode(self, mode: str) -> None:
        self.config.timer_mode = mode
        self.reset()

    def adjust_countdown(self, delta: float) -> None:
        """config.countdown_duration was changed by `delta` seconds; keep
        the live remaining time in sync (running) or reload it (stopped)."""
        if self.running:
            self._cd_remaining += delta
        else:
            self._cd_remaining = float(self.config.countdown_duration)

    def _phase_duration(self) -> float:
        if self._phase == "rest":
            return float(self.config.interval_rest)
        return float(self.config.interval_work)

    def _advance_phase(self):
        """Move to the next interval phase. Returns (beep | None, session_done)."""
        if self._phase == "work":
            if self._round >= self.config.interval_rounds:
                self.running = False
                self._cd_remaining = 0.0
                return Beep(), True   # long finish beep at session end
            if self.config.interval_rest > 0:
                self._phase = "rest"
                self._cd_remaining = float(self.config.interval_rest)
            else:
                self._round += 1
                self._cd_remaining = float(self.config.interval_work)
            self._start_mono = self._clock()
            self._last_short_beep_sec = -1
            return (Beep(double=True) if self._phase == "rest" else Beep()), False
        self._phase = "work"
        self._round += 1
        self._cd_remaining = float(self.config.interval_work)
        self._start_mono = self._clock()
        self._last_short_beep_sec = -1
        return Beep(), False

    # ── Tick ───────────────────────────────────────────────────────────
    def _cycling(self) -> bool:
        """Stopwatch counting upward through work/rest cycles."""
        return self.config.timer_mode == "stopwatch" and self.config.stopwatch_work > 0

    def _cycle_boundaries(self, elapsed: float) -> int:
        """Phase boundaries passed by `elapsed` (work-ends and rest-ends)."""
        work = self.config.stopwatch_work
        rest = self.config.stopwatch_rest
        cycle = work + rest
        full = int(elapsed // cycle)
        pos = elapsed - full * cycle
        if rest > 0:
            return full * 2 + (1 if pos >= work else 0)
        return full

    def tick(self) -> TickResult:
        beeps: list[Beep] = []
        finished = False
        restarted = False
        cyc_phase = ""
        cyc_round = 0
        cyc_progress = None

        # Warn window: sound_alert_before seconds before the next interval beep
        warn = False
        if (
            self.running
            and self.config.timer_mode != "interval"
            and not self._cycling()
            and self.config.sound_enabled
            and self.config.sound_alert_before > 0
            and self.config.sound_interval > 0
        ):
            ref = self.elapsed()
            next_beep = (int(ref / self.config.sound_interval) + 1) * self.config.sound_interval
            warn = next_beep - self.config.sound_alert_before <= ref < next_beep

        if self.config.timer_mode == "stopwatch":
            display = self.elapsed()
            state = "run" if self.running else "pause"
            if state == "run" and warn:
                state = "warn"
            if self._cycling():
                work = self.config.stopwatch_work
                rest = self.config.stopwatch_rest
                cycle = work + rest
                pos = display % cycle if cycle > 0 else 0.0
                in_work = pos < work
                cyc_phase = "work" if in_work else "rest"
                cyc_round = int(display // cycle) + 1
                phase_dur = work if in_work else rest
                phase_remaining = (work - pos) if in_work else (cycle - pos)
                if phase_dur > 0:
                    cyc_progress = max(0.0, min(1.0, phase_remaining / phase_dur))
                if self.running:
                    # Boundary beeps: double when rest starts, long when work starts
                    marks = self._cycle_boundaries(display)
                    if marks > self._cycle_beats:
                        self._cycle_beats = marks
                        rest_start = rest > 0 and marks % 2 == 1
                        beeps.append(Beep(double=True) if rest_start else Beep())
                        self._last_short_beep_sec = -1
                    # Last-5 shorts before each boundary, on the label grid
                    if self.config.alert_last_5_seconds and phase_remaining <= 5.0:
                        state = "warn"
                        sec = int(math.ceil(phase_remaining))
                        if sec != self._last_short_beep_sec and 1 <= sec <= 5:
                            self._last_short_beep_sec = sec
                            beeps.append(Beep(short=True))
        elif self.config.timer_mode == "interval":
            remaining = self.remaining()
            display = max(0.0, remaining)
            if remaining <= 0:
                if self.running:
                    transition_beep, done = self._advance_phase()
                    if transition_beep is not None:
                        beeps.append(transition_beep)
                    if done:
                        finished = True
                        state = "end"
                    else:
                        remaining = self.remaining()
                        display = max(0.0, remaining)
                        state = "run"
                else:
                    state = "end"     # finished session parked at 00:00
            elif self.running and remaining <= 5.0 and self.config.alert_last_5_seconds:
                # Ceil matches the countdown label (which ceils), so the first
                # short lands exactly as the display flips to 5.
                sec_display = int(math.ceil(remaining))
                state = "warn"
                if sec_display != self._last_short_beep_sec and 1 <= sec_display <= 5:
                    self._last_short_beep_sec = sec_display
                    beeps.append(Beep(short=True))
            else:
                state = "run" if self.running else "pause"
        else:
            remaining = self.remaining()
            display = max(0.0, remaining)
            sec_display = int(math.ceil(display))   # matches the ceiled label
            if remaining <= 0:
                state = "end"
                if self.running:
                    finished = True
                    beeps.append(Beep())   # long beep exactly when time runs out
                    if self.config.auto_restart_countdown:
                        self._cd_remaining = float(self.config.countdown_duration)
                        self._start_mono = self._clock()
                        self._sound_beats = 0
                        self._last_short_beep_sec = -1
                        restarted = True
                    else:
                        self.running = False
            elif self.running and remaining <= 5.0 and self.config.alert_last_5_seconds:
                state = "end" if sec_display == 1 else "warn"
                if sec_display != self._last_short_beep_sec and 1 <= sec_display <= 5:
                    self._last_short_beep_sec = sec_display
                    beeps.append(Beep(short=True))
            else:
                state = "run" if self.running else "pause"
                if state == "run" and warn and remaining > 6.0:
                    state = "warn"

        # Periodic interval beeps (not while a cycle structure provides beeps)
        if (
            self.running
            and self.config.timer_mode != "interval"
            and not self._cycling()
            and self.config.sound_enabled
            and self.config.sound_interval > 0
        ):
            ref = self.elapsed()
            if self.config.sound_alert_before > 0:
                target = (
                    (self._sound_alert_before_beats + 1) * self.config.sound_interval
                    - self.config.sound_alert_before
                )
                if target > 0 and ref >= target:
                    self._sound_alert_before_beats += 1
                    beeps.append(Beep(double=True))
            beats = int(ref / self.config.sound_interval)
            if beats > self._sound_beats:
                self._sound_beats = beats
                beeps.append(Beep())

        progress = -1.0
        if self.config.timer_mode == "countdown" and self.config.countdown_duration > 0:
            progress = max(0.0, min(1.0, display / self.config.countdown_duration))
        elif self.config.timer_mode == "interval":
            dur = self._phase_duration()
            if dur > 0:
                progress = max(0.0, min(1.0, display / dur))

        in_interval = self.config.timer_mode == "interval"
        if cyc_progress is not None:
            progress = cyc_progress
        return TickResult(
            display=display, state=state, beeps=beeps,
            finished=finished, restarted=restarted,
            phase=self._phase if in_interval else cyc_phase,
            round=self._round if in_interval else cyc_round,
            rounds=self.config.interval_rounds if in_interval else 0,
            progress=progress,
        )
