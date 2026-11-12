//! Beep playback: generated tones, optional user sound file, player fallback.
//!
//! `Sound::play` never blocks: it sends a message to one worker thread that
//! lazily opens the rodio output, drops it after 10 s idle, and falls back to
//! an external player (paplay/aplay/ffplay/mpv) if no device can be opened.

use std::cell::Cell;
use std::f64::consts::PI;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use rodio::buffer::SamplesBuffer;
use rodio::{
    ChannelCount, Decoder, DeviceSinkBuilder, MixerDeviceSink, Player, SampleRate, Source as _,
};

use crate::config::Config;
use crate::engine::Beep;

pub const SAMPLE_RATE: u32 = 44100;
const IDLE_RELEASE: Duration = Duration::from_secs(10);
const PLAYER_TIMEOUT: Duration = Duration::from_secs(5);
/// First wait before retrying to open the audio device after a failure;
/// doubles per further failure up to [`OPEN_BACKOFF_MAX`].
const OPEN_BACKOFF: Duration = Duration::from_secs(5);
const OPEN_BACKOFF_MAX: Duration = Duration::from_secs(300);

/// Port of `_get_beep`: 16-bit mono sine with a short attack/release envelope.
pub fn beep_samples(freq: f32, dur: f32) -> Vec<i16> {
    let (freq, dur) = (freq as f64, dur as f64);
    let rate = SAMPLE_RATE as f64;
    let n = (rate * dur) as usize;
    (0..n)
        .map(|i| {
            let t = i as f64 / rate;
            let attack = (t / 0.008).min(1.0);
            let release = ((dur - t) / 0.025).min(1.0);
            (20000.0 * attack * release * (2.0 * PI * freq * t).sin()) as i16
        })
        .collect()
}

/// Port of `_get_double_beep`: beep, 0.1 s silence, beep.
pub fn double_beep_samples() -> Vec<i16> {
    let one = beep_samples(880.0, 0.1);
    let mut out = one.clone();
    out.extend(std::iter::repeat_n(0, (SAMPLE_RATE / 10) as usize));
    out.extend(one);
    out
}

/// 16-bit mono PCM WAV file image.
pub fn wav_bytes(samples: &[i16]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + data_len as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    w.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // byte rate
    w.extend_from_slice(&2u16.to_le_bytes()); // block align
    w.extend_from_slice(&16u16.to_le_bytes()); // bits
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        w.extend_from_slice(&s.to_le_bytes());
    }
    w
}

/// Built-in sound set, from `sound_theme`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SoundTheme {
    /// The 880 Hz beeps (identical to the Python app).
    #[default]
    Classic,
    /// Soft bell-like tones.
    Chime,
}

impl SoundTheme {
    /// `"chime"` selects the chimes; anything else plays classic.
    pub fn from_name(name: &str) -> SoundTheme {
        if name == "chime" {
            SoundTheme::Chime
        } else {
            SoundTheme::Classic
        }
    }
}

/// One bell-like note at `start` s within a `total`-second buffer: a few
/// partials decaying exponentially (upper ones faster), 5 ms attack and a
/// 20 ms fade at the note's end so nothing clicks.
fn add_chime_note(out: &mut [f64], start: f64, freq: f64, dur: f64, amp: f64) {
    const PARTIALS: [(f64, f64); 3] = [(1.0, 1.0), (2.0, 0.25), (3.0, 0.08)];
    let rate = SAMPLE_RATE as f64;
    let first = (start * rate) as usize;
    let n = ((dur * rate) as usize).min(out.len().saturating_sub(first));
    let tau = dur / 5.0;
    for i in 0..n {
        let t = i as f64 / rate;
        let attack = (t / 0.005).min(1.0);
        let fade = ((dur - t) / 0.02).min(1.0);
        let v: f64 = PARTIALS
            .iter()
            .map(|(ratio, gain)| {
                gain * (-t * ratio / tau).exp() * (2.0 * PI * freq * ratio * t).sin()
            })
            .sum();
        out[first + i] += amp * attack * fade * v;
    }
}

/// The chime set: a soft C6 "ting" (short), a rising E5/A5 chime (long),
/// two quick tings (double).
pub fn chime_samples(beep: Beep) -> Vec<i16> {
    let (total, notes): (f64, &[(f64, f64, f64, f64)]) = if beep.double {
        (
            0.37,
            &[(0.0, 1046.5, 0.22, 10000.0), (0.15, 1046.5, 0.22, 10000.0)],
        )
    } else if beep.short {
        (0.30, &[(0.0, 1046.5, 0.30, 11000.0)])
    } else {
        (
            1.2,
            &[(0.0, 659.3, 1.2, 9000.0), (0.15, 880.0, 1.05, 9000.0)],
        )
    };
    let mut out = vec![0.0; (SAMPLE_RATE as f64 * total) as usize];
    for &(start, freq, dur, amp) in notes {
        add_chime_note(&mut out, start, freq, dur, amp);
    }
    let peak = out.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    let scale = if peak > 14000.0 { 14000.0 / peak } else { 1.0 };
    out.iter().map(|v| (v * scale) as i16).collect()
}

fn builtin_samples(beep: Beep, theme: SoundTheme) -> Vec<i16> {
    if theme == SoundTheme::Chime {
        chime_samples(beep)
    } else if beep.double {
        double_beep_samples()
    } else if beep.short {
        beep_samples(880.0, 0.1)
    } else {
        beep_samples(880.0, 0.5)
    }
}

/// Cache file name of a built-in sound for the player fallback.
fn wav_name(beep: Beep, theme: SoundTheme) -> String {
    match theme {
        SoundTheme::Classic => format!("timehud-beep-{}.wav", beep_name(beep)),
        SoundTheme::Chime => format!("timehud-beep-chime-{}.wav", beep_name(beep)),
    }
}

fn beep_name(beep: Beep) -> &'static str {
    if beep.double {
        "double"
    } else if beep.short {
        "short"
    } else {
        "long"
    }
}

/// A decoded user sound file (16-bit, at most stereo, within
/// [`MAX_CLIP_SECS`] and [`MAX_CLIP_BYTES`]), shared by every play through
/// `Arc` so nothing is copied per beep.
#[derive(Debug)]
pub struct Decoded {
    channels: ChannelCount,
    rate: SampleRate,
    samples: Vec<i16>,
}

impl Decoded {
    fn duration(&self) -> Duration {
        clip_duration(self.samples.len(), self.channels, self.rate)
    }
}

/// Longest `sound_file` audio kept in memory and played.
const MAX_CLIP_SECS: usize = 30;
/// Memory cap for the decoded clip (~17 s of 48 kHz stereo, 30 s of mono).
const MAX_CLIP_BYTES: usize = 3 << 20;

/// Frames kept of a clip with `channels` (after downmixing) at `rate`:
/// [`MAX_CLIP_SECS`], or less if that exceeds [`MAX_CLIP_BYTES`].
fn max_frames(channels: u16, rate: SampleRate) -> usize {
    let frame_bytes = 2 * usize::from(channels);
    (MAX_CLIP_SECS * rate.get() as usize).min(MAX_CLIP_BYTES / frame_bytes)
}

/// f32 sample in [-1, 1] to 16-bit.
fn to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

/// Converts interleaved f32 `samples` with `channels` to 16-bit, downmixing
/// more than two channels to stereo (even channels left, odd right) and
/// keeping at most `max_frames` frames. Returns the samples, the output
/// channel count and whether audio was cut.
fn compact(
    mut samples: impl Iterator<Item = f32>,
    channels: u16,
    max_frames: usize,
) -> (Vec<i16>, u16, bool) {
    let channels = channels.max(1);
    let out_ch = channels.min(2);
    let mut out = Vec::new();
    let mut frame = Vec::with_capacity(usize::from(channels));
    let mut frames = 0;
    loop {
        frame.clear();
        frame.extend(samples.by_ref().take(usize::from(channels)));
        if frame.is_empty() {
            return (out, out_ch, false);
        }
        if frames == max_frames {
            return (out, out_ch, true);
        }
        if channels <= 2 {
            out.extend(frame.iter().map(|s| to_i16(*s)));
            // A trailing partial frame is padded so channels stay aligned.
            out.extend(std::iter::repeat_n(0, usize::from(channels) - frame.len()));
        } else {
            for side in 0..2 {
                let ch: Vec<f32> = frame.iter().skip(side).step_by(2).copied().collect();
                let mean = ch.iter().sum::<f32>() / ch.len().max(1) as f32;
                out.push(to_i16(mean));
            }
        }
        frames += 1;
    }
}

/// Play time of `len` interleaved samples.
fn clip_duration(len: usize, channels: ChannelCount, rate: SampleRate) -> Duration {
    let frames = len as f64 / f64::from(channels.get());
    Duration::from_secs_f64(frames / f64::from(rate.get()))
}

/// The device may be released once [`IDLE_RELEASE`] has passed since the
/// last clip *ended* (`busy_until`), so long clips are never cut off.
fn release_due(busy_until: Instant, now: Instant) -> bool {
    now >= busy_until + IDLE_RELEASE
}

/// A rodio source reading a shared [`Decoded`] without copying it.
struct SharedSamples {
    decoded: Arc<Decoded>,
    pos: usize,
}

impl SharedSamples {
    fn new(decoded: Arc<Decoded>) -> SharedSamples {
        SharedSamples { decoded, pos: 0 }
    }
}

impl Iterator for SharedSamples {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let s = self.decoded.samples.get(self.pos).copied()?;
        self.pos += 1;
        Some(f32::from(s) / 32768.0)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.decoded.samples.len() - self.pos;
        (left, Some(left))
    }
}

impl rodio::Source for SharedSamples {
    fn current_span_len(&self) -> Option<usize> {
        // Same convention as rodio's `SamplesBuffer`: one span, 0 when done.
        let len = self.decoded.samples.len();
        Some(if self.pos >= len { 0 } else { len })
    }

    fn channels(&self) -> ChannelCount {
        self.decoded.channels
    }

    fn sample_rate(&self) -> SampleRate {
        self.decoded.rate
    }

    fn total_duration(&self) -> Option<Duration> {
        Some(self.decoded.duration())
    }
}

#[derive(Debug)]
pub enum Source {
    Builtin(Beep, SoundTheme),
    File(PathBuf, Arc<Decoded>),
}

type CacheEntry = (PathBuf, Option<SystemTime>, Option<Arc<Decoded>>);
static FILE_CACHE: Mutex<Option<CacheEntry>> = Mutex::new(None);

fn decode_file(path: &Path) -> Result<Decoded, String> {
    let dec = Decoder::try_from(File::open(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let (channels, rate) = (dec.channels().get(), dec.sample_rate());
    let max = max_frames(channels.min(2), rate);
    let (samples, out_ch, cut) = compact(dec, channels, max);
    if samples.is_empty() {
        return Err("no samples".into());
    }
    let channels = ChannelCount::new(out_ch).expect("at least one channel");
    if cut {
        // Logged once: the result is cached by path + mtime.
        let secs = clip_duration(samples.len(), channels, rate).as_secs_f32();
        eprintln!(
            "timehud: sound_file {} is too long; playing only the first {secs:.1} s",
            path.display()
        );
    }
    Ok(Decoded {
        channels,
        rate,
        samples,
    })
}

/// Drops the cached clip (sound disabled or no `sound_file`).
fn forget_file() {
    *FILE_CACHE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Decode `path`, cached by path + mtime (failures are cached too, so the
/// error is logged once and the file is not re-probed on every beep).
fn load_file(path: &Path) -> Option<Arc<Decoded>> {
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let mut cache = FILE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((p, m, d)) = cache.as_ref()
        && p == path
        && *m == mtime
    {
        return d.clone();
    }
    // Release the previous clip before decoding the new one.
    *cache = None;
    let decoded = match decode_file(path) {
        Ok(d) => Some(Arc::new(d)),
        Err(e) => {
            eprintln!(
                "timehud: cannot decode sound_file {}: {e}; using built-in beep",
                path.display()
            );
            None
        }
    };
    *cache = Some((path.to_path_buf(), mtime, decoded.clone()));
    decoded
}

/// Pick what to play: the configured file if it decodes, else the built-in beep.
pub fn resolve_source(cfg: &Config, beep: Beep) -> Source {
    resolve_file(
        &cfg.sound_file,
        beep,
        SoundTheme::from_name(&cfg.sound_theme),
    )
}

fn resolve_file(sound_file: &str, beep: Beep, theme: SoundTheme) -> Source {
    let path = Path::new(sound_file);
    if sound_file.is_empty() || !path.exists() {
        forget_file();
    } else if let Some(d) = load_file(path) {
        return Source::File(path.to_path_buf(), d);
    }
    Source::Builtin(beep, theme)
}

enum Msg {
    Play(Beep, String, SoundTheme),
    /// Sound was disabled: drop the cached clip.
    Forget,
}

/// How long to wait before the next device open after `failures`
/// consecutive failed opens (at least 1).
fn open_backoff(failures: u32) -> Duration {
    OPEN_BACKOFF
        .saturating_mul(1 << failures.saturating_sub(1).min(16))
        .min(OPEN_BACKOFF_MAX)
}

/// Worker-side state: the lazily opened device and when its last clip ends.
struct Worker {
    sink: Option<MixerDeviceSink>,
    busy_until: Instant,
    /// Consecutive failed device opens, and when the next open may be tried.
    open_failures: u32,
    retry_at: Option<Instant>,
}

impl Worker {
    fn new() -> Worker {
        Worker {
            sink: None,
            busy_until: Instant::now(),
            open_failures: 0,
            retry_at: None,
        }
    }

    /// When the open device is due for release; None while closed (the
    /// worker then blocks without a timeout).
    fn release_at(&self) -> Option<Instant> {
        self.sink.as_ref().map(|_| self.busy_until + IDLE_RELEASE)
    }

    fn idle_release(&mut self) {
        if self.sink.is_some() && release_due(self.busy_until, Instant::now()) {
            self.sink = None;
        }
    }

    /// Handles messages until the `Sound` is dropped, waking only to
    /// release the device once it has been idle for [`IDLE_RELEASE`].
    fn run(mut self, rx: Receiver<Msg>) {
        loop {
            let msg = match self.release_at() {
                Some(at) => match rx.recv_timeout(at.saturating_duration_since(Instant::now())) {
                    Ok(msg) => msg,
                    Err(RecvTimeoutError::Timeout) => {
                        self.idle_release();
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => return,
                },
                None => match rx.recv() {
                    Ok(msg) => msg,
                    Err(_) => return,
                },
            };
            match msg {
                Msg::Play(beep, file, theme) => {
                    if let Err(e) = self.play(beep, &file, theme) {
                        eprintln!("timehud: sound failed: {e}");
                    }
                }
                Msg::Forget => forget_file(),
            }
        }
    }

    /// Opens the default device unless a failed open is still backing off.
    /// Only the first failure of a streak is logged.
    fn open_sink(&mut self) -> bool {
        let now = Instant::now();
        if self.retry_at.is_some_and(|t| now < t) {
            return false;
        }
        match DeviceSinkBuilder::open_default_sink() {
            Ok(mut s) => {
                s.log_on_drop(false);
                self.sink = Some(s);
                self.open_failures = 0;
                self.retry_at = None;
                true
            }
            Err(e) => {
                self.open_failures += 1;
                if self.open_failures == 1 {
                    eprintln!("timehud: audio device unavailable ({e}); using player fallback");
                }
                self.retry_at = Some(now + open_backoff(self.open_failures));
                false
            }
        }
    }

    /// Play through rodio, or an external player if no device opens.
    /// Returns the name of the path used.
    fn play(
        &mut self,
        beep: Beep,
        sound_file: &str,
        theme: SoundTheme,
    ) -> Result<&'static str, String> {
        let source = resolve_file(sound_file, beep, theme);
        if self.sink.is_none() && !self.open_sink() {
            return play_with_player(&source).map(|()| "player");
        }
        let sink = self.sink.as_ref().expect("sink just opened");
        let player = Player::connect_new(sink.mixer());
        let length = match source {
            Source::File(_, d) => {
                let length = d.duration();
                player.append(SharedSamples::new(d));
                length
            }
            Source::Builtin(b, theme) => {
                let f: Vec<f32> = builtin_samples(b, theme)
                    .into_iter()
                    .map(|s| s as f32 / 32768.0)
                    .collect();
                let buf = SamplesBuffer::new(
                    ChannelCount::new(1).unwrap(),
                    SampleRate::new(SAMPLE_RATE).unwrap(),
                    f,
                );
                let length = buf.total_duration().unwrap_or_default();
                player.append(buf);
                length
            }
        };
        player.detach();
        // Keep the device open at least until this clip has finished.
        self.busy_until = self.busy_until.max(Instant::now() + length);
        Ok("rodio")
    }
}

/// Port of `_play`: write the WAV once, run the first available player.
fn play_with_player(source: &Source) -> Result<(), String> {
    let path = match source {
        Source::File(p, _) => p.clone(),
        Source::Builtin(b, theme) => {
            let dir = std::env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            let p = dir.join(wav_name(*b, *theme));
            if !p.exists() {
                std::fs::write(&p, wav_bytes(&builtin_samples(*b, *theme)))
                    .map_err(|e| e.to_string())?;
            }
            p
        }
    };
    let players: [(&str, &[&str]); 4] = [
        ("paplay", &[]),
        ("aplay", &[]),
        ("ffplay", &["-nodisp", "-autoexit", "-loglevel", "quiet"]),
        ("mpv", &["--no-video", "--really-quiet"]),
    ];
    for (prog, args) in players {
        let child = Command::new(prog)
            .args(args)
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(mut child) = child {
            // Reap (and kill if hung) off the worker thread.
            thread::spawn(move || {
                let start = Instant::now();
                while start.elapsed() < PLAYER_TIMEOUT {
                    if !matches!(child.try_wait(), Ok(None)) {
                        return;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
                let _ = child.kill();
                let _ = child.wait();
            });
            return Ok(());
        }
    }
    Err("no audio player found".into())
}

pub struct Sound {
    tx: Sender<Msg>,
    /// Whether the worker may hold a decoded `sound_file` (last play named one).
    file_cached: Cell<bool>,
}

impl Default for Sound {
    fn default() -> Self {
        Self::new()
    }
}

impl Sound {
    /// Starts the worker thread; opens no audio device.
    pub fn new() -> Sound {
        let (tx, rx) = channel::<Msg>();
        thread::Builder::new()
            .name("timehud-sound".into())
            .spawn(move || Worker::new().run(rx))
            .expect("spawn sound thread");
        Sound {
            tx,
            file_cached: Cell::new(false),
        }
    }

    /// Non-blocking; plays nothing when `sound_enabled` is off (and then
    /// releases a cached `sound_file` clip).
    pub fn play(&self, beep: Beep, cfg: &Config) {
        if cfg.sound_enabled {
            let theme = SoundTheme::from_name(&cfg.sound_theme);
            let _ = self.tx.send(Msg::Play(beep, cfg.sound_file.clone(), theme));
            self.file_cached.set(!cfg.sound_file.is_empty());
        } else {
            self.config_changed(cfg);
        }
    }

    /// Releases a cached `sound_file` clip right away once nothing can play
    /// it (sound off or no file), instead of at the next beep.
    pub fn config_changed(&self, cfg: &Config) {
        if (!cfg.sound_enabled || cfg.sound_file.is_empty()) && self.file_cached.replace(false) {
            let _ = self.tx.send(Msg::Forget);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LONG: Beep = Beep {
        short: false,
        double: false,
    };
    const SHORT: Beep = Beep {
        short: true,
        double: false,
    };
    const DOUBLE: Beep = Beep {
        short: false,
        double: true,
    };

    fn peak(s: &[i16]) -> i32 {
        s.iter().map(|v| i32::from(*v).abs()).max().unwrap()
    }

    fn secs(s: &[i16]) -> f64 {
        s.len() as f64 / f64::from(SAMPLE_RATE)
    }

    #[test]
    fn classic_is_unchanged() {
        let classic = |b| builtin_samples(b, SoundTheme::Classic);
        assert_eq!(classic(SHORT), beep_samples(880.0, 0.1));
        assert_eq!(classic(LONG), beep_samples(880.0, 0.5));
        assert_eq!(classic(DOUBLE), double_beep_samples());
    }

    #[test]
    fn theme_from_name_is_tolerant() {
        assert_eq!(SoundTheme::from_name("chime"), SoundTheme::Chime);
        for n in ["classic", "Chime", "bell", ""] {
            assert_eq!(SoundTheme::from_name(n), SoundTheme::Classic);
        }
    }

    #[test]
    fn chime_short_fits_tick_spacing() {
        assert!(secs(&chime_samples(SHORT)) <= 0.4, "decays well within 1 s");
    }

    #[test]
    fn chime_lengths() {
        let long = secs(&chime_samples(LONG));
        assert!((1.0..=1.5).contains(&long), "long {long}");
        let double = secs(&chime_samples(DOUBLE));
        assert!(
            double > secs(&chime_samples(SHORT)) && double <= 0.7,
            "double {double}"
        );
    }

    #[test]
    fn chime_is_softer_and_click_free() {
        for b in [SHORT, LONG, DOUBLE] {
            let s = chime_samples(b);
            assert!(peak(&s) <= 14000, "softer than classic's 20000");
            assert!(peak(&s) >= 6000, "still audible");
            assert!(i32::from(s[0]).abs() < 100, "silent start");
            let tail = &s[s.len() - SAMPLE_RATE as usize / 100..]; // last 10 ms
            assert!(peak(tail) < 200, "decayed tail, no click");
        }
    }

    #[test]
    fn missing_file_falls_back_to_themed_builtin() {
        let _guard = CACHE_TEST.lock().unwrap_or_else(|e| e.into_inner());
        assert!(matches!(
            resolve_file("/nonexistent/x.wav", LONG, SoundTheme::Chime),
            Source::Builtin(_, SoundTheme::Chime)
        ));
    }

    #[test]
    fn fallback_wav_names_differ_per_theme() {
        assert_eq!(wav_name(LONG, SoundTheme::Classic), "timehud-beep-long.wav");
        for b in [SHORT, LONG, DOUBLE] {
            assert_ne!(
                wav_name(b, SoundTheme::Classic),
                wav_name(b, SoundTheme::Chime)
            );
        }
    }

    #[test]
    fn play_sends_current_theme() {
        let (tx, rx) = channel::<Msg>();
        let s = Sound {
            tx,
            file_cached: Cell::new(false),
        };
        let cfg = Config {
            sound_theme: "chime".into(),
            ..Config::default()
        };
        s.play(LONG, &cfg);
        assert!(matches!(
            rx.try_recv(),
            Ok(Msg::Play(_, _, SoundTheme::Chime))
        ));
    }

    #[test]
    fn long_beep_len() {
        assert_eq!(beep_samples(880., 0.5).len(), 22050);
    }

    #[test]
    fn envelope_starts_and_ends_silent() {
        let s = beep_samples(880., 0.5);
        assert!(s[0].abs() < 100);
        assert!(s[s.len() - 1].abs() < 1500);
    }

    #[test]
    fn peak_amplitude() {
        let s = beep_samples(880., 0.5);
        let peak = s.iter().map(|v| (*v as i32).abs()).max().unwrap();
        assert!((19000..=20000).contains(&peak), "peak {peak}");
    }

    #[test]
    fn double_beep_len() {
        assert_eq!(double_beep_samples().len(), 4410 + 4410 + 4410);
    }

    #[test]
    fn wav_header() {
        let s = beep_samples(880., 0.1);
        let w = wav_bytes(&s);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(&w[8..12], b"WAVE");
        let data_size = u32::from_le_bytes(w[40..44].try_into().unwrap());
        assert_eq!(data_size as usize, 2 * s.len());
        assert_eq!(w.len(), 44 + 2 * s.len());
    }

    #[test]
    fn bad_sound_file_falls_back() {
        let _guard = CACHE_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let mut cfg = Config {
            sound_file: "/nonexistent.ogg".into(),
            ..Config::default()
        };
        assert!(matches!(
            resolve_source(&cfg, Beep::default()),
            Source::Builtin(..)
        ));
        let dir = tempfile::tempdir().unwrap();
        let txt = dir.path().join("not_audio.ogg");
        std::fs::write(&txt, "this is not audio").unwrap();
        cfg.sound_file = txt.to_string_lossy().into_owned();
        assert!(matches!(
            resolve_source(&cfg, Beep::default()),
            Source::Builtin(..)
        ));
    }

    #[test]
    fn valid_sound_file_is_used() {
        let _guard = CACHE_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("ok.wav");
        std::fs::write(&wav, wav_bytes(&beep_samples(440., 0.1))).unwrap();
        let cfg = Config {
            sound_file: wav.to_string_lossy().into_owned(),
            ..Config::default()
        };
        assert!(matches!(
            resolve_source(&cfg, Beep::default()),
            Source::File(..)
        ));
    }

    #[test]
    fn decode_cap_is_thirty_seconds_or_three_megabytes() {
        let rate = SampleRate::new(48000).unwrap();
        // Mono 48 kHz: 30 s is 2.9 MB of i16.
        assert_eq!(max_frames(1, rate), 30 * 48000);
        // Stereo 48 kHz: the byte cap wins (~16 s).
        assert_eq!(max_frames(2, rate), MAX_CLIP_BYTES / 4);
        assert!(max_frames(2, rate) * 4 <= MAX_CLIP_BYTES);
    }

    #[test]
    fn compact_converts_and_truncates_only_beyond_the_limit() {
        let ramp = || [0.0f32, 0.5, -0.5, 1.0, -1.0, 2.0].into_iter();
        let (s, ch, cut) = compact(ramp(), 1, 4);
        assert_eq!((s, ch, cut), (vec![0, 16384, -16384, 32767], 1, true));
        let (s, ch, cut) = compact(ramp(), 2, 3);
        assert_eq!(s.len(), 6);
        assert_eq!((ch, cut), (2, false));
        assert_eq!(s[5], 32767, "clamped");
    }

    #[test]
    fn compact_downmixes_to_stereo() {
        // Two 4-channel frames: (L, R, L, R).
        let frames = [0.5f32, -0.5, 0.25, -0.25, 0.0, 0.0, 1.0, 1.0];
        let (s, ch, cut) = compact(frames.into_iter(), 4, 10);
        assert_eq!((ch, cut), (2, false));
        assert_eq!(
            s,
            vec![to_i16(0.375), to_i16(-0.375), to_i16(0.5), to_i16(0.5)]
        );
    }

    #[test]
    fn compact_pads_a_trailing_partial_frame() {
        let (s, ch, _) = compact([0.5f32, 0.5, 0.5].into_iter(), 2, 10);
        assert_eq!((s.len(), ch), (4, 2));
        assert_eq!(s[3], 0);
    }

    /// Serializes tests that inspect the process-wide [`FILE_CACHE`].
    static CACHE_TEST: Mutex<()> = Mutex::new(());

    fn cached_path() -> Option<PathBuf> {
        let cache = FILE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        cache.as_ref().map(|(p, _, _)| p.clone())
    }

    #[test]
    fn cached_clip_is_dropped_when_the_file_is_unset_or_changed() {
        let _guard = CACHE_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.wav"), dir.path().join("b.wav"));
        for p in [&a, &b] {
            std::fs::write(p, wav_bytes(&beep_samples(440., 0.1))).unwrap();
        }
        let s = |p: &Path| p.to_string_lossy().into_owned();
        assert!(matches!(
            resolve_file(&s(&a), Beep::default(), SoundTheme::Classic),
            Source::File(..)
        ));
        assert_eq!(cached_path(), Some(a.clone()));
        let Source::File(_, d) = resolve_file(&s(&b), Beep::default(), SoundTheme::Classic) else {
            panic!("b decodes");
        };
        assert_eq!(cached_path(), Some(b.clone()));
        assert_eq!(d.samples.len(), 4410, "16-bit mono kept as is");
        resolve_file("", Beep::default(), SoundTheme::Classic);
        assert_eq!(cached_path(), None);
        resolve_file(&s(&a), Beep::default(), SoundTheme::Classic);
        resolve_file(
            &s(&dir.path().join("gone.wav")),
            Beep::default(),
            SoundTheme::Classic,
        );
        assert_eq!(cached_path(), None, "a missing file drops the clip too");
    }

    #[test]
    fn clip_duration_from_frames() {
        let stereo = ChannelCount::new(2).unwrap();
        let rate = SampleRate::new(44100).unwrap();
        assert_eq!(
            clip_duration(44100 * 2 * 3, stereo, rate),
            Duration::from_secs(3)
        );
        assert_eq!(
            clip_duration(22050, ChannelCount::new(1).unwrap(), rate),
            Duration::from_millis(500)
        );
    }

    #[test]
    fn release_waits_for_clip_end_plus_idle() {
        let start = Instant::now();
        // A 25 s clip started at `start`: still playing at +20 s.
        let busy_until = start + Duration::from_secs(25);
        assert!(!release_due(busy_until, start + Duration::from_secs(20)));
        assert!(!release_due(busy_until, start + Duration::from_secs(34)));
        assert!(release_due(busy_until, start + Duration::from_secs(35)));
    }

    #[test]
    fn device_open_backs_off_exponentially() {
        assert_eq!(open_backoff(1), Duration::from_secs(5));
        assert_eq!(open_backoff(2), Duration::from_secs(10));
        assert_eq!(open_backoff(4), Duration::from_secs(40));
        assert_eq!(open_backoff(7), OPEN_BACKOFF_MAX);
        assert_eq!(open_backoff(u32::MAX), OPEN_BACKOFF_MAX);
    }

    #[test]
    fn closed_device_needs_no_wakeup() {
        let mut w = Worker::new();
        assert_eq!(w.release_at(), None);
        w.idle_release();
        assert!(w.sink.is_none());
    }

    #[test]
    fn config_change_releases_the_clip_once_unplayable() {
        let (tx, rx) = channel::<Msg>();
        let s = Sound {
            tx,
            file_cached: Cell::new(false),
        };
        let on = Config {
            sound_enabled: true,
            sound_file: "/x.wav".into(),
            ..Config::default()
        };
        s.play(Beep::default(), &on);
        assert!(matches!(rx.try_recv(), Ok(Msg::Play(..))));
        s.config_changed(&on);
        assert!(rx.try_recv().is_err(), "still playable: kept");
        for off in [
            Config {
                sound_enabled: false,
                ..on.clone()
            },
            Config {
                sound_file: String::new(),
                ..on.clone()
            },
        ] {
            s.play(Beep::default(), &on);
            assert!(matches!(rx.try_recv(), Ok(Msg::Play(..))));
            s.config_changed(&off);
            assert!(matches!(rx.try_recv(), Ok(Msg::Forget)));
            s.config_changed(&off);
            assert!(rx.try_recv().is_err(), "forgotten once");
        }
    }

    #[test]
    fn worker_exits_when_sound_is_dropped() {
        let _guard = CACHE_TEST.lock().unwrap_or_else(|e| e.into_inner());
        let (tx, rx) = channel::<Msg>();
        let t = thread::spawn(move || Worker::new().run(rx));
        tx.send(Msg::Forget).unwrap();
        drop(tx);
        t.join().unwrap();
    }

    #[test]
    fn shared_samples_play_without_copying() {
        let d = Arc::new(Decoded {
            channels: ChannelCount::new(1).unwrap(),
            rate: SampleRate::new(SAMPLE_RATE).unwrap(),
            samples: vec![3277, 6554, 9830],
        });
        let a = SharedSamples::new(d.clone());
        let b = SharedSamples::new(d.clone());
        assert!(Arc::ptr_eq(&a.decoded, &d) && Arc::ptr_eq(&b.decoded, &d));
        assert_eq!(Arc::strong_count(&d), 3);
        assert_eq!(a.total_duration(), Some(d.duration()));
        let played: Vec<f32> = b.collect();
        assert_eq!(played.len(), 3);
        for (got, want) in played.iter().zip([0.1, 0.2, 0.3]) {
            assert!((got - want).abs() < 1e-3, "{got} vs {want}");
        }
    }

    /// Manual: plays long, short, double of each theme through the real
    /// output path. Run with
    /// `cargo test smoke_play_beeps -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn smoke_play_beeps() {
        let mut w = Worker::new();
        for theme in [SoundTheme::Classic, SoundTheme::Chime] {
            for (beep, wait) in [(LONG, 1500), (SHORT, 1000), (DOUBLE, 1000)] {
                let via = w.play(beep, "", theme).expect("play failed");
                println!("played {theme:?} {} via {via}", beep_name(beep));
                std::thread::sleep(Duration::from_millis(wait));
            }
        }
        // And through the public, threaded API.
        let s = Sound::new();
        s.play(Beep::default(), &Config::default());
        std::thread::sleep(Duration::from_millis(700));
    }
}
