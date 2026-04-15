"""
sound_manager.py – Sound alert playback for TimeHUD.

Uses only the Python standard library for WAV generation, then delegates
to whatever audio player is available on the system (paplay → aplay → ffplay).
No additional Python packages required.
"""

import math
import os
import struct
import subprocess
import tempfile
import threading
import wave


class SoundManager:
    def __init__(self, config) -> None:
        self.config = config
        self._beeps = {}

    # ── Public API ─────────────────────────────────────────────────────────

    def play_alert(self, short: bool = False) -> None:
        """Play the configured (or default) alert in a background thread."""
        if not self.config.sound_enabled:
            return

        sound_file = self.config.sound_file
        if not sound_file or not os.path.exists(sound_file):
            if short:
                sound_file = self._get_beep(880, 0.1)
            else:
                sound_file = self._get_beep(880, 0.5)

        threading.Thread(target=self._play, args=(sound_file,), daemon=True).start()

    def cleanup(self) -> None:
        """Remove the temp beep files on exit."""
        for path in self._beeps.values():
            try:
                if os.path.exists(path):
                    os.unlink(path)
            except OSError:
                pass

    # ── Internal helpers ───────────────────────────────────────────────────

    def _get_beep(self, frequency=880, duration=0.5) -> str:
        """Generate (once) a beep WAV and return its path."""
        key = (frequency, duration)
        if key in self._beeps and os.path.exists(self._beeps[key]):
            return self._beeps[key]

        tmp = tempfile.NamedTemporaryFile(suffix=".wav", delete=False)
        tmp.close()
        path = tmp.name
        self._beeps[key] = path

        sample_rate = 44100
        n_samples = int(sample_rate * duration)

        with wave.open(path, "w") as wf:
            wf.setnchannels(1)
            wf.setsampwidth(2)           # 16-bit PCM
            wf.setframerate(sample_rate)
            for i in range(n_samples):
                t   = i / sample_rate
                # Smooth attack/release envelope so there's no click
                attack  = min(1.0, t / 0.008)
                release = min(1.0, (duration - t) / 0.025)
                env = attack * release
                sample = int(20000 * env * math.sin(2 * math.pi * frequency * t))
                wf.writeframes(struct.pack("<h", sample))

        return path

    @staticmethod
    def _play(path: str) -> None:
        """Try several common Linux audio players in order."""
        players = [
            ["paplay", path],
            ["aplay",  path],
            ["ffplay", "-nodisp", "-autoexit", "-loglevel", "quiet", path],
            ["mpv",    "--no-video", "--really-quiet", path],
        ]
        for cmd in players:
            try:
                subprocess.run(cmd, timeout=5, capture_output=True)
                return          # success – stop trying
            except (FileNotFoundError, subprocess.TimeoutExpired):
                continue        # player not found / hung – try next
