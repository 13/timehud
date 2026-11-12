# Chime sound theme — design (short)

## Intent

The built-in beep (flat 880 Hz sine at high amplitude, hard edges) is too
harsh. Add a softer alternative: a "chime" sound set, selectable next to the
existing beep. The classic beep stays the default and unchanged.

## Goals / non-goals

- Goal: a `sound_theme` setting with `"classic"` (today's beeps, bit-identical)
  and `"chime"` (soft bell-like tones), chosen in Settings → Sound.
- Goal: every beep kind keeps its meaning and stays distinguishable:
  short (last-5 ticks), long (countdown/phase end), double (halfway /
  `sound_alert_before`).
- Non-goal: changing beep timing, which beeps fire, or `sound_file` behaviour
  (a set `sound_file` still overrides every built-in sound, whatever the theme).
- Non-goal: bundled audio files.
- Anti-goal: chime ticks that blur into each other (the last-5 shorts are 1 s
  apart) or a chime so quiet it's missed.

## Approaches

1. **Synthesize chimes in code** — *picked*. Same mechanism as the classic
   beep; no assets, no licensing, ~nothing added to binary or idle RSS; the
   player fallback already writes generated sounds to WAV.
2. Bundle recorded chime samples — more natural, but licensing, binary size,
   decode cost, and it duplicates what `sound_file` already offers.
3. Soften the existing beep — smallest, but removes the classic sound, which
   must stay identical to the Python app's.

## Design

- **Config**: new field `sound_theme: String = "classic"` in `config_fields!`,
  plus the README settings-JSON example. Unknown values play classic (same
  tolerant pattern as `theme`). The Python app ignores the unknown key, and
  old configs load with the default.
- **Synthesis** (`sound.rs`): `builtin_samples(beep, theme)`. Chime voice =
  sine fundamental + a couple of quieter inharmonic/harmonic partials, ~5 ms
  attack, exponential decay, peak amplitude well below classic's 20000.
  - short: single soft "ting" (~C6/1047 Hz), decays out in ~0.3 s.
  - long: two-note chime (rising, e.g. E5 → A5 overlapping), ~1.2 s tail.
  - double: two quick tings ~0.15 s apart.
  Exact pitches/partials/levels are tuned by ear during implementation.
- **Plumbing**: the worker's `Msg::Play` carries the theme with the
  `sound_file`; the player-fallback WAV cache name includes the theme
  (`timehud-beep-chime-long.wav`) so classic/chime files don't collide.
- **Settings → Sound**: a "Sound" choice (Classic beep / Soft chime) with a
  ▶ preview button that plays the long sound of the selected theme in the
  settings process itself (same binary, `sound.rs`), so the user can hear
  it before choosing. Disabled-looking note when `sound_file` is set
  ("custom sound overrides this").
- **Docs**: README Sound section mentions the setting.
- **Tests**: classic samples unchanged (pin length/peak/checksum of current
  output); chime samples are shorter than the next-tick spacing for short,
  have lower peak than classic, decay to ~0 at the end (no click); config
  round-trip and unknown-value fallback.

## Left to the builder

Exact tone recipe (frequencies, partial ratios, decay constants, level), as
long as the constraints above hold.
