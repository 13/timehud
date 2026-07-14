import pytest

from timehud.timer_engine import TimerEngine, fmt_seconds


@pytest.fixture
def engine(config, clock):
    return TimerEngine(config, clock=clock)


class TestStopwatch:
    def test_starts_at_zero_paused(self, engine):
        assert engine.running is False
        assert engine.elapsed() == 0.0

    def test_accumulates_while_running(self, engine, clock):
        engine.toggle()
        clock.advance(12.5)
        assert engine.elapsed() == pytest.approx(12.5)

    def test_pause_freezes_and_resume_continues(self, engine, clock):
        engine.toggle()
        clock.advance(10)
        engine.toggle()          # pause
        clock.advance(99)        # time passes while paused
        assert engine.elapsed() == pytest.approx(10)
        engine.toggle()          # resume
        clock.advance(5)
        assert engine.elapsed() == pytest.approx(15)

    def test_reset_zeroes(self, engine, clock):
        engine.toggle()
        clock.advance(30)
        engine.reset()
        assert engine.running is False
        assert engine.elapsed() == 0.0

    def test_tick_state_run_and_pause(self, engine, clock):
        assert engine.tick().state == "pause"
        engine.toggle()
        clock.advance(3)
        r = engine.tick()
        assert r.state == "run"
        assert r.display == pytest.approx(3)


class TestCountdown:
    @pytest.fixture
    def engine(self, config, clock):
        config.timer_mode = "countdown"
        config.countdown_duration = 60
        return TimerEngine(config, clock=clock)

    def test_counts_down(self, engine, clock):
        engine.toggle()
        clock.advance(25)
        assert engine.remaining() == pytest.approx(35)
        assert engine.tick().display == pytest.approx(35)

    def test_pause_resume(self, engine, clock):
        engine.toggle()
        clock.advance(20)
        engine.toggle()
        clock.advance(500)
        assert engine.remaining() == pytest.approx(40)

    def test_finish_stops_engine(self, engine, clock):
        engine.toggle()
        clock.advance(61)
        r = engine.tick()
        assert r.state == "end"
        assert r.finished is True
        assert r.display == 0.0
        assert engine.running is False
        # subsequent ticks stay ended but don't re-report finished
        assert engine.tick().finished is False

    def test_auto_restart(self, engine, clock):
        engine.config.auto_restart_countdown = True
        engine.toggle()
        clock.advance(61)
        r = engine.tick()
        assert r.restarted is True
        assert engine.running is True
        assert engine.remaining() == pytest.approx(60)

    def test_start_after_finish_reloads_duration(self, engine, clock):
        engine.toggle()
        clock.advance(61)
        engine.tick()
        engine.toggle()          # restart from full duration
        assert engine.remaining() == pytest.approx(60)

    def test_adjust_countdown_while_running(self, engine, clock):
        engine.toggle()
        clock.advance(10)
        engine.config.countdown_duration += 60
        engine.adjust_countdown(60)
        assert engine.remaining() == pytest.approx(110)

    def test_adjust_countdown_while_stopped_reloads(self, engine):
        engine.config.countdown_duration = 90
        engine.adjust_countdown(30)
        assert engine.remaining() == pytest.approx(90)


class TestModeSwitch:
    def test_set_mode_resets(self, engine, clock):
        engine.toggle()
        clock.advance(42)
        engine.set_mode("countdown")
        assert engine.config.timer_mode == "countdown"
        assert engine.running is False
        assert engine.remaining() == pytest.approx(engine.config.countdown_duration)


def collect_beeps(engine, clock, seconds, step=0.1):
    """Advance in `step` increments, ticking like the 100 ms UI loop."""
    out = []
    n = int(seconds / step)
    for _ in range(n):
        clock.advance(step)
        out.extend(engine.tick().beeps)
    return out


class TestIntervalBeeps:
    @pytest.fixture
    def engine(self, config, clock):
        config.timer_mode = "stopwatch"
        config.sound_enabled = True
        config.sound_interval = 60
        config.sound_alert_before = 0
        return TimerEngine(config, clock=clock)

    def test_main_beep_every_interval(self, engine, clock):
        engine.toggle()
        beeps = collect_beeps(engine, clock, 125)
        mains = [b for b in beeps if not b.short and not b.double]
        assert len(mains) == 2          # at 60 s and 120 s

    def test_no_beeps_when_sound_disabled(self, engine, clock):
        engine.config.sound_enabled = False
        engine.toggle()
        assert collect_beeps(engine, clock, 125) == []

    def test_no_beeps_while_paused(self, engine, clock):
        assert collect_beeps(engine, clock, 125) == []

    def test_alert_before_double_beep(self, engine, clock):
        engine.config.sound_alert_before = 5
        engine.reset()
        engine.toggle()
        beeps = collect_beeps(engine, clock, 59)
        doubles = [b for b in beeps if b.double]
        assert len(doubles) == 1        # at 55 s (60 − 5)
        beeps = collect_beeps(engine, clock, 3)  # through 62 s
        mains = [b for b in beeps if not b.short and not b.double]
        assert len(mains) == 1          # main beep still at 60 s

    def test_pause_resume_does_not_replay_past_beeps(self, engine, clock):
        engine.toggle()
        collect_beeps(engine, clock, 70)   # beep at 60 consumed
        engine.toggle()                    # pause at 70 s
        engine.toggle()                    # resume
        beeps = collect_beeps(engine, clock, 5)
        assert beeps == []                 # nothing until 120 s


class TestLastFiveSeconds:
    @pytest.fixture
    def engine(self, config, clock):
        config.timer_mode = "countdown"
        config.countdown_duration = 30
        config.sound_enabled = True
        config.sound_interval = 6000       # keep interval beeps out of the way
        config.alert_last_5_seconds = True
        return TimerEngine(config, clock=clock)

    def test_short_beeps_then_long_at_true_zero(self, engine, clock):
        engine.toggle()
        beeps = collect_beeps(engine, clock, 29.95)   # label shows 00:00, not done
        shorts = [b for b in beeps if b.short]
        longs = [b for b in beeps if not b.short and not b.double]
        assert len(shorts) == 5            # displayed 5,4,3,2,1
        assert longs == []                 # nothing during the displayed-zero second
        beeps = collect_beeps(engine, clock, 0.2)     # cross true zero (30 s)
        longs = [b for b in beeps if not b.short and not b.double]
        assert len(longs) == 1             # long beep exactly when time runs out

    def test_first_short_fires_when_label_shows_five(self, engine, clock):
        engine.toggle()
        clock.advance(25.05)               # remaining 4.95 → ceiled label "00:05"
        beeps = engine.tick().beeps
        assert [b for b in beeps if b.short], "short beep must start at displayed 5"

    def test_no_short_while_label_shows_six(self, engine, clock):
        engine.toggle()
        clock.advance(24.4)                # remaining 5.6 → ceiled label "00:06"
        assert engine.tick().beeps == []

    def test_finish_beep_without_last5_flag(self, engine, clock):
        engine.config.alert_last_5_seconds = False
        engine.reset()
        engine.toggle()
        beeps = collect_beeps(engine, clock, 30.2)
        assert [b for b in beeps if b.short] == []
        longs = [b for b in beeps if not b.short and not b.double]
        assert len(longs) == 1             # finish beep independent of last-5 flag

    def test_warn_then_end_state(self, engine, clock):
        engine.toggle()
        clock.advance(26.5)                # remaining 3.5 → displayed 4
        assert engine.tick().state == "warn"
        clock.advance(2.6)                 # remaining 0.9 → displayed 1
        assert engine.tick().state == "end"

    def test_disabled_flag_suppresses(self, engine, clock):
        engine.config.alert_last_5_seconds = False
        engine.toggle()
        beeps = collect_beeps(engine, clock, 29.5)
        assert [b for b in beeps if b.short] == []


class TestIsIdle:
    def test_idle_at_reset(self, engine):
        assert engine.is_idle() is True

    def test_not_idle_while_running(self, engine, clock):
        engine.toggle()
        clock.advance(1)
        assert engine.is_idle() is False

    def test_not_idle_when_paused_midway(self, engine, clock):
        engine.toggle()
        clock.advance(5)
        engine.toggle()
        assert engine.is_idle() is False

    def test_idle_again_after_reset(self, engine, clock):
        engine.toggle()
        clock.advance(5)
        engine.reset()
        assert engine.is_idle() is True

    def test_idle_countdown_at_full_duration(self, config, clock):
        config.timer_mode = "countdown"
        config.countdown_duration = 60
        from timehud.timer_engine import TimerEngine
        e = TimerEngine(config, clock=clock)
        assert e.is_idle() is True
        e.toggle()
        clock.advance(3)
        e.toggle()
        assert e.is_idle() is False


class TestInterval:
    @pytest.fixture
    def engine(self, config, clock):
        config.timer_mode = "interval"
        config.interval_work = 40
        config.interval_rest = 20
        config.interval_rounds = 3
        config.sound_enabled = True
        config.sound_interval = 60           # must NOT fire in interval mode
        config.alert_last_5_seconds = False
        return TimerEngine(config, clock=clock)

    def test_idle_start(self, engine):
        assert engine.is_idle() is True
        r = engine.tick()
        assert r.phase == "work" and r.round == 1 and r.rounds == 3
        assert r.display == pytest.approx(40)

    def test_work_to_rest_transition(self, engine, clock):
        engine.toggle()
        clock.advance(40.05)
        r = engine.tick()
        assert r.phase == "rest" and r.round == 1
        assert [b for b in r.beeps if not b.double and not b.short], "phase end = long beep"
        assert r.display == pytest.approx(20, abs=0.1)

    def test_rest_to_work_advances_round(self, engine, clock):
        engine.toggle()
        clock.advance(40.05)
        engine.tick()
        clock.advance(20.05)
        r = engine.tick()
        assert r.phase == "work" and r.round == 2
        assert [b for b in r.beeps if not b.short and not b.double]

    def test_session_finish_skips_last_rest(self, engine, clock):
        engine.toggle()
        # 3 rounds: W R W R W -> end (no trailing rest)
        for _ in range(2):
            clock.advance(40.05)
            engine.tick()   # work ends
            clock.advance(20.05)
            engine.tick()   # rest ends
        clock.advance(40.05)
        r = engine.tick()
        assert r.finished is True and r.state == "end"
        assert engine.running is False
        # parked at zero afterwards
        r2 = engine.tick()
        assert r2.state == "end" and r2.display == 0.0

    def test_zero_rest_back_to_back(self, engine, clock):
        engine.config.interval_rest = 0
        engine.reset()
        engine.toggle()
        clock.advance(40.05)
        r = engine.tick()
        assert r.phase == "work" and r.round == 2
        assert [b for b in r.beeps if not b.double and not b.short]

    def test_no_periodic_beeps_in_interval(self, engine, clock):
        engine.toggle()
        beeps = collect_beeps(engine, clock, 39)   # crosses nothing, sound_interval=60 would... stay silent
        assert beeps == []

    def test_last5_shorts_no_long(self, engine, clock):
        engine.config.alert_last_5_seconds = True
        engine.reset()
        engine.toggle()
        beeps = collect_beeps(engine, clock, 39.5)
        shorts = [b for b in beeps if b.short]
        longs = [b for b in beeps if not b.short and not b.double]
        assert len(shorts) == 5      # displayed 5,4,3,2,1
        assert longs == []           # transition beep covers the zero mark

    def test_pause_resume_mid_phase(self, engine, clock):
        engine.toggle()
        clock.advance(10)
        engine.toggle()
        clock.advance(500)
        assert engine.remaining() == pytest.approx(30)
        assert engine.is_idle() is False

    def test_interval_halfway_double_in_work(self, engine, clock):
        engine.config.halfway_beep = True
        engine.reset()
        engine.toggle()
        beeps = collect_beeps(engine, clock, 39)      # work phase (40 s, half = 20)
        doubles = [b for b in beeps if b.double]
        assert len(doubles) == 1                      # halfway at 20 s

    def test_interval_transitions_all_long(self, engine, clock):
        engine.toggle()
        beeps = collect_beeps(engine, clock, 61)      # work→rest at 40, rest→work at 60
        longs = [b for b in beeps if not b.short and not b.double]
        assert len(longs) == 2
        assert [b for b in beeps if b.double] == []

    def test_interval_phase_beeps_toggle_off(self, engine, clock):
        engine.config.phase_beeps = False
        engine.reset()
        engine.toggle()
        beeps = collect_beeps(engine, clock, 61)      # crosses work→rest→work
        assert [b for b in beeps if b.double] == []
        assert [b for b in beeps if not b.short and not b.double] == []

    def test_restart_after_finish(self, engine, clock):
        engine.toggle()
        for _ in range(2):
            clock.advance(40.05)
            engine.tick()
            clock.advance(20.05)
            engine.tick()
        clock.advance(40.05)
        engine.tick()      # finished
        engine.toggle()                           # start again from round 1
        r = engine.tick()
        assert engine.running is True
        assert r.phase == "work" and r.round == 1
        assert r.display == pytest.approx(40, abs=0.1)


class TestZeroSoundInterval:
    """sound_interval == 0 disables periodic beeps without crashing."""

    def test_stopwatch_no_periodic_no_crash(self, config, clock):
        config.timer_mode = "stopwatch"
        config.sound_enabled = True
        config.sound_interval = 0
        config.sound_alert_before = 5
        e = TimerEngine(config, clock=clock)
        e.toggle()
        assert collect_beeps(e, clock, 120) == []

    def test_countdown_finish_beep_still_fires(self, config, clock):
        config.timer_mode = "countdown"
        config.countdown_duration = 10
        config.sound_enabled = True
        config.sound_interval = 0
        e = TimerEngine(config, clock=clock)
        e.toggle()
        beeps = collect_beeps(e, clock, 10.2)
        longs = [b for b in beeps if not b.short and not b.double]
        assert len(longs) == 1


class TestCyclingStopwatch:
    @pytest.fixture
    def engine(self, config, clock):
        config.timer_mode = "stopwatch"
        config.stopwatch_work = 45
        config.stopwatch_rest = 15
        config.sound_enabled = True
        config.sound_interval = 60           # must NOT fire while cycling
        config.alert_last_5_seconds = False
        return TimerEngine(config, clock=clock)

    def test_counts_upward_with_phase_info(self, engine, clock):
        engine.toggle()
        clock.advance(10)
        r = engine.tick()
        assert r.display == pytest.approx(10)       # count-up display
        assert r.phase == "work" and r.round == 1
        assert r.rounds == 0                        # endless
        clock.advance(40)                           # elapsed 50 → rest of round 1
        r = engine.tick()
        assert r.display == pytest.approx(50)
        assert r.phase == "rest" and r.round == 1

    def test_round_increments_each_cycle(self, engine, clock):
        engine.toggle()
        clock.advance(61)                           # into round 2 work
        r = engine.tick()
        assert r.phase == "work" and r.round == 2

    def test_boundary_beeps_all_long(self, engine, clock):
        engine.toggle()
        beeps = collect_beeps(engine, clock, 121)
        longs = [b for b in beeps if not b.double and not b.short]
        assert len(longs) == 4                      # 45, 60, 105, 120 s
        assert [b for b in beeps if b.double] == [] # double is halfway-only

    def test_zero_rest_long_beeps_only(self, engine, clock):
        engine.config.stopwatch_rest = 0
        engine.reset()
        engine.toggle()
        beeps = collect_beeps(engine, clock, 100)
        assert [b for b in beeps if b.double] == []
        longs = [b for b in beeps if not b.double and not b.short]
        assert len(longs) == 2                      # work ends at 45s and 90s

    def test_no_periodic_interval_beeps_while_cycling(self, engine, clock):
        engine.toggle()
        beeps = collect_beeps(engine, clock, 44)    # before first boundary
        assert beeps == []                          # sound_interval=60 stays quiet

    def test_last5_shorts_before_each_boundary(self, engine, clock):
        engine.config.alert_last_5_seconds = True
        engine.reset()
        engine.toggle()
        beeps = collect_beeps(engine, clock, 44.5)  # work phase, 0.5s left
        shorts = [b for b in beeps if b.short]
        assert len(shorts) == 5                     # phase-remaining 5..1

    def test_progress_tracks_phase_remaining(self, engine, clock):
        engine.toggle()
        clock.advance(40)                           # 5s left in 45s work
        r = engine.tick()
        assert r.progress == pytest.approx(5 / 45, abs=0.01)
        clock.advance(10)                           # 10s left in 15s rest
        r = engine.tick()
        assert r.progress == pytest.approx(10 / 15, abs=0.01)

    def test_pause_resume_does_not_replay_boundaries(self, engine, clock):
        engine.toggle()
        collect_beeps(engine, clock, 50)            # double at 45 consumed
        engine.toggle()                             # pause in rest
        engine.toggle()                             # resume
        beeps = collect_beeps(engine, clock, 5)     # elapsed 55, next boundary 60
        assert beeps == []

    def test_halfway_double_in_each_work_phase(self, engine, clock):
        engine.config.halfway_beep = True
        engine.reset()
        engine.toggle()
        beeps = collect_beeps(engine, clock, 44)      # work phase (45 s, half = 22.5)
        doubles = [b for b in beeps if b.double]
        assert len(doubles) == 1                      # halfway at 22.5 s
        beeps = collect_beeps(engine, clock, 15)      # rest: no halfway cue
        assert [b for b in beeps if b.double] == []
        beeps = collect_beeps(engine, clock, 25)      # round 2 work halfway at 82.5
        assert len([b for b in beeps if b.double]) == 1

    def test_halfway_not_replayed_after_pause(self, engine, clock):
        engine.config.halfway_beep = True
        engine.reset()
        engine.toggle()
        collect_beeps(engine, clock, 25)              # halfway at 22.5 consumed
        engine.toggle()
        engine.toggle()
        beeps = collect_beeps(engine, clock, 3)
        assert [b for b in beeps if b.double] == []

    def test_halfway_off_by_default(self, engine, clock):
        engine.toggle()
        beeps = collect_beeps(engine, clock, 44)
        assert [b for b in beeps if b.double] == []

    def test_rest_phase_keeps_run_state_outside_last5(self, engine, clock):
        engine.config.halfway_beep = True
        engine.config.alert_last_5_seconds = True
        engine.reset()
        engine.toggle()
        clock.advance(47)                             # rest, 13 s remaining
        r = engine.tick()
        assert r.phase == "rest" and r.state == "run" # blue rest, no warn tint

    def test_phase_beeps_toggle_off(self, engine, clock):
        engine.config.phase_beeps = False
        engine.reset()
        engine.toggle()
        beeps = collect_beeps(engine, clock, 121)     # two full cycles
        assert [b for b in beeps if b.double or (not b.short and not b.double)] == []

    def test_plain_stopwatch_unchanged(self, config, clock):
        config.timer_mode = "stopwatch"
        config.stopwatch_work = 0
        e = TimerEngine(config, clock=clock)
        e.toggle()
        clock.advance(10)
        r = e.tick()
        assert r.phase == "" and r.round == 0
        assert r.progress == -1.0


class TestProgress:
    def test_stopwatch_has_no_progress(self, engine, clock):
        engine.toggle()
        clock.advance(5)
        assert engine.tick().progress == -1.0

    def test_countdown_progress(self, config, clock):
        config.timer_mode = "countdown"
        config.countdown_duration = 100
        e = TimerEngine(config, clock=clock)
        e.toggle()
        clock.advance(25)
        assert e.tick().progress == pytest.approx(0.75)

    def test_interval_progress_per_phase(self, config, clock):
        config.timer_mode = "interval"
        config.interval_work = 40
        config.interval_rest = 20
        config.interval_rounds = 2
        e = TimerEngine(config, clock=clock)
        e.toggle()
        clock.advance(30)
        assert e.tick().progress == pytest.approx(0.25)   # 10/40 left
        clock.advance(10.05)
        e.tick()                                          # into rest
        clock.advance(5)
        assert e.tick().progress == pytest.approx(15 / 20, abs=0.01)


class TestFmtSeconds:
    def test_under_a_minute(self):
        assert fmt_seconds(59) == "00:59"

    def test_minutes(self):
        assert fmt_seconds(61) == "01:01"
        assert fmt_seconds(300) == "05:00"

    def test_hour_boundary(self):
        assert fmt_seconds(3599) == "59:59"
        assert fmt_seconds(3600) == "01:00:00"

    def test_large(self):
        assert fmt_seconds(86399) == "23:59:59"

    def test_negative_clamps_to_zero(self):
        assert fmt_seconds(-3) == "00:00"
