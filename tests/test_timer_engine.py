import pytest

from timehud.timer_engine import TimerEngine


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
