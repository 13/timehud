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
use std::sync::mpsc::{Sender, channel};
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

fn builtin_samples(beep: Beep) -> Vec<i16> {
    if beep.double {
        double_beep_samples()
    } else if beep.short {
        beep_samples(880.0, 0.1)
    } else {
        beep_samples(880.0, 0.5)
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
    Builtin(Beep),
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
    resolve_file(&cfg.sound_file, beep)
}

fn resolve_file(sound_file: &str, beep: Beep) -> Source {
    let path = Path::new(sound_file);
    if sound_file.is_empty() || !path.exists() {
        forget_file();
    } else if let Some(d) = load_file(path) {
        return Source::File(path.to_path_buf(), d);
    }
    Source::Builtin(beep)
}

enum Msg {
    Play(Beep, String),
    /// Sound was disabled: drop the cached clip.
    Forget,
    Idle,
}

/// Worker-side state: the lazily opened device and when its last clip ends.
struct Worker {
    sink: Option<MixerDeviceSink>,
    busy_until: Instant,
}

impl Worker {
    fn new() -> Worker {
        Worker {
            sink: None,
            busy_until: Instant::now(),
        }
    }

    fn idle_release(&mut self) {
        if self.sink.is_some() && release_due(self.busy_until, Instant::now()) {
            self.sink = None;
        }
    }

    /// Play through rodio, or an external player if no device opens.
    /// Returns the name of the path used.
    fn play(&mut self, beep: Beep, sound_file: &str) -> Result<&'static str, String> {
        let source = resolve_file(sound_file, beep);
        if self.sink.is_none() {
            match DeviceSinkBuilder::open_default_sink() {
                Ok(mut s) => {
                    s.log_on_drop(false);
                    self.sink = Some(s);
                }
                Err(e) => {
                    eprintln!("timehud: audio device unavailable ({e}); using player fallback");
                    return play_with_player(&source).map(|()| "player");
                }
            }
        }
        let sink = self.sink.as_ref().expect("sink just opened");
        let player = Player::connect_new(sink.mixer());
        let length = match source {
            Source::File(_, d) => {
                let length = d.duration();
                player.append(SharedSamples::new(d));
                length
            }
            Source::Builtin(b) => {
                let f: Vec<f32> = builtin_samples(b)
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
        Source::Builtin(b) => {
            let dir = std::env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            let p = dir.join(format!("timehud-beep-{}.wav", beep_name(*b)));
            if !p.exists() {
                std::fs::write(&p, wav_bytes(&builtin_samples(*b))).map_err(|e| e.to_string())?;
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
            .spawn(move || {
                let mut worker = Worker::new();
                while let Ok(msg) = rx.recv() {
                    match msg {
                        Msg::Play(beep, file) => {
                            if let Err(e) = worker.play(beep, &file) {
                                eprintln!("timehud: sound failed: {e}");
                            }
                        }
                        Msg::Forget => forget_file(),
                        Msg::Idle => worker.idle_release(),
                    }
                }
            })
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
            let _ = self.tx.send(Msg::Play(beep, cfg.sound_file.clone()));
            self.file_cached.set(!cfg.sound_file.is_empty());
        } else if self.file_cached.replace(false) {
            let _ = self.tx.send(Msg::Forget);
        }
    }

    /// Call about once per second; the device is dropped after 10 s unused.
    pub fn idle_release(&self) {
        let _ = self.tx.send(Msg::Idle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            Source::Builtin(_)
        ));
        let dir = tempfile::tempdir().unwrap();
        let txt = dir.path().join("not_audio.ogg");
        std::fs::write(&txt, "this is not audio").unwrap();
        cfg.sound_file = txt.to_string_lossy().into_owned();
        assert!(matches!(
            resolve_source(&cfg, Beep::default()),
            Source::Builtin(_)
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
            resolve_file(&s(&a), Beep::default()),
            Source::File(..)
        ));
        assert_eq!(cached_path(), Some(a.clone()));
        let Source::File(_, d) = resolve_file(&s(&b), Beep::default()) else {
            panic!("b decodes");
        };
        assert_eq!(cached_path(), Some(b.clone()));
        assert_eq!(d.samples.len(), 4410, "16-bit mono kept as is");
        resolve_file("", Beep::default());
        assert_eq!(cached_path(), None);
        resolve_file(&s(&a), Beep::default());
        resolve_file(&s(&dir.path().join("gone.wav")), Beep::default());
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

    /// Manual: plays long, short, double through the real output path.
    /// Run with `cargo test smoke_play_beeps -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn smoke_play_beeps() {
        let mut w = Worker::new();
        for (beep, wait) in [
            (Beep::default(), 700),
            (
                Beep {
                    short: true,
                    double: false,
                },
                300,
            ),
            (
                Beep {
                    short: false,
                    double: true,
                },
                500,
            ),
        ] {
            let via = w.play(beep, "").expect("play failed");
            println!("played {} via {via}", beep_name(beep));
            std::thread::sleep(Duration::from_millis(wait));
        }
        // And through the public, threaded API.
        let s = Sound::new();
        s.play(Beep::default(), &Config::default());
        s.idle_release();
        std::thread::sleep(Duration::from_millis(700));
    }
}
