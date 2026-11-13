//! System sound (WASAPI loopback of an output device) and the microphone, through cpal.
//!
//! Each side keeps timestamped chunks for the last 120 s; a save rebuilds its window on the real
//! timeline (clip_recorder::render). cpal lists devices afresh on every call, so a device plugged
//! in after launch is found, and its streams handle COM on their own thread.

use crate::logging::log;
use clip_recorder::clock;
use clip_recorder::heal::{Decision, Retry};
use clip_recorder::render::{self, Chunk, render_window};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// History kept per side, whatever the clip length (the longest buffer option).
pub const KEEP_SECONDS: u32 = 120;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Loopback,
    Mic,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Loopback => "loopback",
            Kind::Mic => "mic",
        }
    }
}

type History = Arc<Mutex<VecDeque<(f64, Vec<i16>)>>>;

struct Side {
    kind: Kind,
    /// What config.json asks for: empty means the Windows default.
    configured: String,
    device: Option<cpal::Device>,
    name: String,
    rate: u32,
    channels: u16,
    stream: Option<cpal::Stream>,
    /// Set by the stream's error callback (device unplugged, invalidated by sleep/resume...).
    dead: Arc<AtomicBool>,
    frames: Arc<AtomicU64>,
    history: History,
    retry: Retry,
}

impl Side {
    fn new(kind: Kind, configured: &str) -> Side {
        Side {
            kind,
            configured: clip_recorder::settings::configured_device(configured).to_owned(),
            device: None,
            name: String::new(),
            rate: 48000,
            channels: 2,
            stream: None,
            dead: Arc::new(AtomicBool::new(false)),
            frames: Arc::new(AtomicU64::new(0)),
            history: Arc::new(Mutex::new(VecDeque::new())),
            retry: Retry::default(),
        }
    }

    /// The configured device if it is there, else the Windows default (as the Python app did).
    fn detect(&mut self) {
        let host = cpal::default_host();
        let devices = match self.kind {
            Kind::Loopback => host.output_devices(),
            Kind::Mic => host.input_devices(),
        };
        let mut listed: Vec<cpal::Device> = devices.map(|ds| ds.collect()).unwrap_or_default();
        let names: Vec<String> = listed.iter().map(device_name).collect();
        let wanted = clip_recorder::settings::pick_device(&names, &self.configured).map(|i| listed.swap_remove(i));
        let device = wanted.or_else(|| match self.kind {
            Kind::Loopback => host.default_output_device(),
            Kind::Mic => host.default_input_device(),
        });
        let config = device.as_ref().and_then(|d| match self.kind {
            Kind::Loopback => d.default_output_config().ok(),
            Kind::Mic => d.default_input_config().ok(),
        });
        match (device, config) {
            (Some(d), Some(c)) => {
                self.name = device_name(&d);
                self.rate = c.sample_rate();
                self.channels = c.channels();
                self.device = Some(d);
            }
            _ => {
                log(&format!("no {} device found", self.kind.label()));
                self.device = None;
            }
        }
    }

    fn alive(&self) -> bool {
        self.stream.is_some() && !self.dead.load(Ordering::Relaxed)
    }

    fn open(&mut self) {
        self.stream = None;
        let Some(device) = self.device.clone() else { return };
        let config = match self.kind {
            Kind::Loopback => device.default_output_config(),
            Kind::Mic => device.default_input_config(),
        };
        let Ok(config) = config else {
            log(&format!(
                "{} stream OPEN FAILED: no config for {}",
                self.kind.label(),
                self.name
            ));
            return;
        };
        self.dead = Arc::new(AtomicBool::new(false));
        self.frames = Arc::new(AtomicU64::new(0));
        let built = match config.sample_format() {
            SampleFormat::F32 => self.build::<f32>(&device, config.config()),
            SampleFormat::I16 => self.build::<i16>(&device, config.config()),
            SampleFormat::I32 => self.build::<i32>(&device, config.config()),
            SampleFormat::U16 => self.build::<u16>(&device, config.config()),
            other => {
                log(&format!(
                    "{} stream OPEN FAILED: sample format {other:?} not supported",
                    self.kind.label()
                ));
                return;
            }
        };
        match built.and_then(|s| s.play().map(|_| s)) {
            Ok(stream) => {
                log(&format!(
                    "{} stream opened: device={:?} rate={} channels={}",
                    self.kind.label(),
                    self.name,
                    self.rate,
                    self.channels
                ));
                self.stream = Some(stream);
            }
            Err(e) => log(&format!(
                "{} stream OPEN FAILED: device={:?}: {e}",
                self.kind.label(),
                self.name
            )),
        }
    }

    /// The data callback stamps each chunk with its arrival time and keeps 120 s. Real-time
    /// code: no I/O, no logging, one short lock.
    fn build<T>(&self, device: &cpal::Device, config: cpal::StreamConfig) -> Result<cpal::Stream, cpal::Error>
    where
        T: SizedSample,
        i16: FromSample<T>,
    {
        let history = Arc::clone(&self.history);
        let frames = Arc::clone(&self.frames);
        let dead = Arc::clone(&self.dead);
        let channels = config.channels.max(1) as usize;
        device.build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                let arrival = clock::now();
                let samples: Vec<i16> = data.iter().map(|&s| i16::from_sample_(s)).collect();
                frames.fetch_add((samples.len() / channels) as u64, Ordering::Relaxed);
                let mut history = history.lock().unwrap_or_else(|e| e.into_inner());
                render::push_chunk(&mut history, arrival, samples, KEEP_SECONDS as f64);
            },
            // Only a lost stream counts: cpal also reports Xruns, refused real-time priority and
            // reroutes here, and the stream carries on through those.
            move |err: cpal::Error| {
                use cpal::ErrorKind::*;
                if matches!(
                    err.kind(),
                    DeviceNotAvailable | StreamInvalidated | HostUnavailable | BackendError
                ) {
                    dead.store(true, Ordering::Relaxed);
                }
            },
            None,
        )
    }

    fn clear(&self) {
        self.history.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    /// One health tick: reopen a dead stream, with this side's own backoff.
    fn heal(&mut self) {
        let frames = self.frames.load(Ordering::Relaxed);
        match self.retry.decide(self.device.is_some(), self.alive(), frames) {
            Decision::Keep => {}
            Decision::Reopen => {
                log(&format!(
                    "{} stream not alive (frames={frames}), reopening",
                    self.kind.label()
                ));
                let (format, previous) = ((self.rate, self.channels), self.device.take());
                self.detect(); // found afresh: cpal re-enumerates
                self.device = render::device_after_reopen(previous, self.device.take());
                if !render::keeps_history(format, (self.rate, self.channels)) {
                    self.clear();
                }
                self.open();
            }
            Decision::GiveUp => log(&format!(
                "{} stream still dead after {} reopens, left alone until a device change or restart",
                self.kind.label(),
                clip_recorder::heal::MAX_REOPENS
            )),
        }
    }

    /// Writes this side's window as a WAV. False when nothing overlaps it (a dead stream).
    fn save_wav(&self, path: &Path, end: f64, duration: f64) -> bool {
        // Only the chunks that can reach the window (a chunk ends at its arrival), so the
        // callbacks wait on a short copy, not on 120 s of audio.
        let history: Vec<(f64, Vec<i16>)> = {
            let all = self.history.lock().unwrap_or_else(|e| e.into_inner());
            all.iter().filter(|(t, _)| *t > end - duration - 1.0).cloned().collect()
        };
        let chunks: Vec<Chunk> = history
            .iter()
            .map(|(t, s)| Chunk {
                arrival: *t,
                samples: s,
            })
            .collect();
        let Some(samples) = render_window(&chunks, self.rate, self.channels, end, duration) else {
            return false;
        };
        let spec = hound::WavSpec {
            channels: self.channels,
            sample_rate: self.rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let write = || -> hound::Result<()> {
            let file = crate::winutil::create_new(path).map_err(hound::Error::IoError)?;
            let mut w = hound::WavWriter::new(std::io::BufWriter::new(file), spec)?;
            let mut s = w.get_i16_writer(samples.len() as u32);
            samples.iter().for_each(|&v| s.write_sample(v));
            s.flush()?;
            w.finalize()
        };
        write().is_ok()
    }

    /// One line for the save log: enough to tell silence, a stall and a dead stream apart.
    fn describe(&self) -> String {
        let history = self.history.lock().unwrap_or_else(|e| e.into_inner());
        let newest = history
            .back()
            .map_or("none".to_owned(), |(t, _)| format!("{:.1}s", clock::now() - t));
        format!(
            "{}: device={:?} rate={} alive={} frames_received={} chunks={} newest_age={newest}",
            self.kind.label(),
            self.name,
            self.rate,
            self.alive(),
            self.frames.load(Ordering::Relaxed),
            history.len()
        )
    }
}

fn device_name(d: &cpal::Device) -> String {
    d.description().map(|desc| desc.name().to_string()).unwrap_or_default()
}

pub struct Audio {
    loopback: Side,
    mic: Side,
    running: bool,
}

impl Audio {
    pub fn new(loopback: &str, mic: &str) -> Audio {
        let mut audio = Audio {
            loopback: Side::new(Kind::Loopback, loopback),
            mic: Side::new(Kind::Mic, mic),
            running: false,
        };
        audio.loopback.detect();
        audio.mic.detect();
        audio
    }

    /// Stays running even when nothing opens: heal() keeps trying.
    pub fn start(&mut self) {
        self.running = true;
        self.loopback.open();
        self.mic.open();
    }

    pub fn stop(&mut self) {
        self.running = false;
        self.loopback.stream = None;
        self.mic.stream = None;
    }

    pub fn heal(&mut self) {
        if self.running {
            self.loopback.heal();
            self.mic.heal();
        }
    }

    /// Settings' device change: the same object (one owner), devices detected again. A side
    /// keeps its history unless its format changed: render_window cannot mix two.
    pub fn reload(&mut self, loopback: &str, mic: &str) {
        let running = self.running;
        self.stop();
        for (side, wanted) in [(&mut self.loopback, loopback), (&mut self.mic, mic)] {
            let before = (side.rate, side.channels);
            *side = Side {
                history: Arc::clone(&side.history),
                ..Side::new(side.kind, wanted)
            };
            side.detect();
            if !render::keeps_history(before, (side.rate, side.channels)) {
                side.clear();
            }
        }
        if running {
            self.start();
        }
    }

    pub fn has(&self, kind: Kind) -> bool {
        self.side(kind).device.is_some()
    }

    pub fn save_wav(&self, kind: Kind, path: &Path, end: f64, duration: f64) -> bool {
        self.side(kind).save_wav(path, end, duration)
    }

    pub fn describe(&self, kind: Kind) -> String {
        self.side(kind).describe()
    }

    fn side(&self, kind: Kind) -> &Side {
        match kind {
            Kind::Loopback => &self.loopback,
            Kind::Mic => &self.mic,
        }
    }
}

/// (output devices, input devices) for Settings, listed afresh.
pub fn list_devices() -> (Vec<String>, Vec<String>) {
    fn names(devices: Result<impl Iterator<Item = cpal::Device>, cpal::Error>) -> Vec<String> {
        devices
            .map(|ds| ds.map(|d| device_name(&d)).filter(|n| !n.is_empty()).collect())
            .unwrap_or_default()
    }
    let host = cpal::default_host();
    (names(host.output_devices()), names(host.input_devices()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs real audio devices: `cargo test -- --ignored --nocapture audio_on_this_machine`.
    #[test]
    #[ignore]
    fn audio_on_this_machine() {
        let (outputs, inputs) = list_devices();
        println!("outputs: {outputs:?}\ninputs: {inputs:?}");
        let mut audio = Audio::new("", "");
        audio.start();
        std::thread::sleep(std::time::Duration::from_secs(3));
        println!("{}\n{}", audio.describe(Kind::Loopback), audio.describe(Kind::Mic));
        let wav = std::env::temp_dir().join(format!("crtest_audio_{}.wav", std::process::id()));
        let saved = audio.save_wav(Kind::Loopback, &wav, clock::now(), 2.0);
        let size = std::fs::metadata(&wav).map(|m| m.len()).unwrap_or(0);
        let _ = std::fs::remove_file(&wav);
        audio.stop();
        assert!(saved && size > 0, "loopback WAV not written ({size} bytes)");
    }
}
