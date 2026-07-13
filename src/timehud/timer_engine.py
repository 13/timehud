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
            self._start_mono = self._clock()
            self.running = True
            self._sound_beats = int(self.elapsed() / self.config.sound_interval)
            self._sound_alert_before_beats = int(
                (self.elapsed() + self.config.sound_alert_before)
                / self.config.sound_interval
            )

    def reset(self) -> None:
        self.running = False
        self._elapsed = 0.0
        self._cd_remaining = float(self.config.countdown_duration)
        self._sound_beats = 0
        self._sound_alert_before_beats = 0
        self._last_short_beep_sec = -1

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

    # ── Tick ───────────────────────────────────────────────────────────
    def tick(self) -> TickResult:
        beeps: list[Beep] = []
        finished = False
        restarted = False

        # Warn window: sound_alert_before seconds before the next interval beep
        warn = False
        if self.running and self.config.sound_enabled and self.config.sound_alert_before > 0:
            ref = self.elapsed()
            next_beep = (int(ref / self.config.sound_interval) + 1) * self.config.sound_interval
            warn = next_beep - self.config.sound_alert_before <= ref < next_beep

        if self.config.timer_mode == "stopwatch":
            display = self.elapsed()
            state = "run" if self.running else "pause"
            if state == "run" and warn:
                state = "warn"
        else:
            remaining = self.remaining()
            display = max(0.0, remaining)
            sec_display = int(math.ceil(display))
            if remaining <= 0:
                state = "end"
                if self.running:
                    finished = True
                    if self.config.auto_restart_countdown:
                        self._cd_remaining = float(self.config.countdown_duration)
                        self._start_mono = self._clock()
                        self._sound_beats = 0
                        self._last_short_beep_sec = -1
                        restarted = True
                    else:
                        self.running = False
            elif self.running and remaining <= 6.0 and self.config.alert_last_5_seconds:
                state = "end" if sec_display == 1 else "warn"
                if sec_display != self._last_short_beep_sec and 1 <= sec_display <= 6:
                    self._last_short_beep_sec = sec_display
                    beeps.append(Beep(short=sec_display != 1))
            else:
                state = "run" if self.running else "pause"
                if state == "run" and warn and remaining > 6.0:
                    state = "warn"

        # Periodic interval beeps
        if self.running and self.config.sound_enabled:
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

        return TickResult(
            display=display, state=state, beeps=beeps,
            finished=finished, restarted=restarted,
        )
