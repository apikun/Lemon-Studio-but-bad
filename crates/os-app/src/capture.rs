// Copyright (C) 2026 ItsAlphaNeon
// Copyright (C) 2026 ravenmarkdev-alt
// Copyright (C) 2026 Artig3nce
// SPDX-License-Identifier: GPL-3.0-or-later
// This file is part of Lemon Studio: https://github.com/ItsAlphaNeon/Lemon-Studio

//! Audio input capture (One-click audio recording): the audio device's inputs into a WAV file.
//!
//! An ASIO driver's inputs come in its own callback, with its outputs (`device::asio`); any other device's inputs come
//! from a system input stream opened while recording (`InputStream`). Either way the callback pushes stereo frames into
//! one lock-free ring (`capture`), only while a recording is on, and a writer thread (`Recorder`) drains it to disk. At
//! the end the writer turns what it wrote into a WAV, leaving out the frames before the take's start (`Recorder::finish`).
//!
//! The same callbacks feed the mixer tracks' inputs (`monitor`): while a track takes an input, every input of the device
//! goes into a second ring that the playing engine reads a tick at a time, before the track's effects. A system input
//! device stays open for it while a track takes one (`monitor_inputs`); a take from that device records from it too.
// spec: ui/one-click-recording.md

use std::cell::UnsafeCell;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use os_engine::LIVE_INPUTS;

/// One producer, one consumer ring of samples: the device's callback pushes, the writer thread pops; neither locks.
pub struct Ring {
    buf: Box<[UnsafeCell<f32>]>,
    /// Samples written and read so far (the slot is the count modulo the length).
    head: AtomicUsize,
    tail: AtomicUsize,
}

// Safety: a slot is written only by the producer before `head` moves past it, and read only by the consumer before
// `tail` moves past it; the counts are published with release / acquire.
unsafe impl Sync for Ring {}

impl Ring {
    pub fn new(len: usize) -> Ring {
        Ring { buf: (0..len.max(1)).map(|_| UnsafeCell::new(0.0)).collect(), head: AtomicUsize::new(0), tail: AtomicUsize::new(0) }
    }

    /// Pushes `n` samples, the `k`-th being `at(k)`, as far as there is room; returns how many went in. Producer only.
    pub fn push_with(&self, n: usize, mut at: impl FnMut(usize) -> f32) -> usize {
        let head = self.head.load(Ordering::Relaxed);
        let free = self.buf.len() - (head - self.tail.load(Ordering::Acquire));
        let n = n.min(free);
        for k in 0..n {
            // Safety: the slot is free (the consumer has read past it) and only this producer writes.
            unsafe { *self.buf[(head + k) % self.buf.len()].get() = at(k) };
        }
        self.head.store(head + n, Ordering::Release);
        n
    }

    /// Appends every sample waiting to `out`. Consumer only.
    pub fn pop_into(&self, out: &mut Vec<f32>) {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);
        // Safety: slots between tail and head were written and published by the producer.
        out.extend((tail..head).map(|i| unsafe { *self.buf[i % self.buf.len()].get() }));
        self.tail.store(head, Ordering::Release);
    }

    /// Empties it. Consumer only.
    pub fn clear(&self) {
        self.tail.store(self.head.load(Ordering::Acquire), Ordering::Release);
    }

    /// Samples waiting. Consumer only.
    pub fn len(&self) -> usize {
        self.head.load(Ordering::Acquire) - self.tail.load(Ordering::Relaxed)
    }

    /// Takes the next `n` samples (as many as are waiting) into `out`, or (`keep` false) skips them. Consumer only.
    pub fn take(&self, n: usize, out: &mut Vec<f32>, keep: bool) {
        let tail = self.tail.load(Ordering::Relaxed);
        let n = n.min(self.head.load(Ordering::Acquire) - tail);
        if keep {
            // Safety: as `pop_into`.
            out.extend((tail..tail + n).map(|i| unsafe { *self.buf[i % self.buf.len()].get() }));
        }
        self.tail.store(tail + n, Ordering::Release);
    }
}

/// Which stream feeds the capture or the monitor: the ASIO driver's callback, or a system input stream's number.
pub const ASIO_SOURCE: u32 = 0;

fn next_source() -> u32 {
    static NEXT: AtomicU32 = AtomicU32::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// The capture the device's callbacks feed: whether a recording is on, which inputs it takes, and what came in.
pub struct Capture {
    ring: Ring,
    on: AtomicBool,
    /// The stream it takes from (`ASIO_SOURCE`, or a system input stream's `source`): only one feeds the ring.
    from: AtomicU32,
    /// The first input (0-based) and whether its next one makes a stereo pair (`set_source`).
    first: AtomicU32,
    stereo: AtomicBool,
    /// Stereo frames that came in since `start` (the ones the ring had no room for included), and those lost.
    frames: AtomicU64,
    dropped: AtomicU64,
}

/// About 20 s of stereo at 48 kHz: the writer drains it every few milliseconds.
const RING: usize = 1 << 21;

/// The one capture.
pub fn capture() -> &'static Capture {
    static C: OnceLock<Capture> = OnceLock::new();
    C.get_or_init(|| Capture {
        ring: Ring::new(RING),
        on: AtomicBool::new(false),
        from: AtomicU32::new(ASIO_SOURCE),
        first: AtomicU32::new(0),
        stereo: AtomicBool::new(true),
        frames: AtomicU64::new(0),
        dropped: AtomicU64::new(0),
    })
}

impl Capture {
    /// Starts taking inputs `first` (and `first + 1` when `stereo`) of stream `from` from its next block.
    pub fn start(&self, from: u32, first: u16, stereo: bool) {
        self.on.store(false, Ordering::Release);
        self.ring.clear();
        self.from.store(from, Ordering::Relaxed);
        self.first.store(u32::from(first), Ordering::Relaxed);
        self.stereo.store(stereo, Ordering::Relaxed);
        self.frames.store(0, Ordering::Relaxed);
        self.dropped.store(0, Ordering::Relaxed);
        self.on.store(true, Ordering::Release);
    }

    pub fn stop(&self) {
        self.on.store(false, Ordering::Release);
    }

    pub fn on(&self) -> bool {
        self.on.load(Ordering::Acquire)
    }

    /// The inputs to take: the first, and whether it is a pair.
    pub fn source(&self) -> (usize, bool) {
        (self.first.load(Ordering::Relaxed) as usize, self.stereo.load(Ordering::Relaxed))
    }

    /// Stereo frames that came in since `start`.
    pub fn frames(&self) -> u64 {
        self.frames.load(Ordering::Acquire)
    }

    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Stream `from`'s block of `frames` frames: `at(channel, frame)` is an input's sample. Pushes the chosen inputs
    /// as stereo (one input on both sides). The audio callback calls it; it neither locks nor allocates.
    pub fn push(&self, from: u32, frames: usize, at: impl Fn(usize, usize) -> f32) {
        if !self.on() || self.from.load(Ordering::Relaxed) != from {
            return;
        }
        let (first, stereo) = self.source();
        let second = if stereo { first + 1 } else { first };
        let went = self.ring.push_with(2 * frames, |k| at(if k % 2 == 0 { first } else { second }, k / 2));
        self.dropped.fetch_add(((2 * frames - went) / 2) as u64, Ordering::Relaxed);
        self.frames.fetch_add(frames as u64, Ordering::AcqRel);
    }

    fn drain(&self, out: &mut Vec<f32>) {
        self.ring.pop_into(out);
    }
}

/// The mixer tracks' live inputs (`os_engine::LiveInput`): every input of one device, `LIVE_INPUTS` to a frame.
pub struct Monitor {
    ring: Ring,
    /// Some track takes an input: stream `from`'s callback pushes.
    on: AtomicBool,
    from: AtomicU32,
    /// The reader hands out input once `PRIME` frames wait (at the start, and again after it ran dry).
    primed: AtomicBool,
    /// One reader at a time: a stream fading out may still render while the next one starts.
    reader: Mutex<()>,
    /// The largest block a stream pushed: the reader keeps that much waiting on top of `PRIME`, so a device that hands
    /// its input over in big blocks is not cut back below one and played dry.
    block: std::sync::atomic::AtomicUsize,
}

/// About 0.7 s of eight inputs at 48 kHz.
const MONITOR_RING: usize = LIVE_INPUTS << 15;
/// Frames the reader keeps waiting, so a device's block arriving late is not a dropout (about 20 ms at 48 kHz), and
/// the most it lets wait before skipping back to that (an input running ahead of the output's clock).
const PRIME: usize = 1024;
const MOST: usize = PRIME + 4096;

/// The one monitor.
pub fn monitor() -> &'static Monitor {
    static M: OnceLock<Monitor> = OnceLock::new();
    M.get_or_init(|| Monitor { ring: Ring::new(MONITOR_RING), on: AtomicBool::new(false), from: AtomicU32::new(ASIO_SOURCE), primed: AtomicBool::new(false), reader: Mutex::new(()), block: std::sync::atomic::AtomicUsize::new(0) })
}

impl Monitor {
    pub fn on(&self) -> bool {
        self.on.load(Ordering::Acquire)
    }

    /// Stream `from`'s block of `frames` frames (`at(channel, frame)`, an input past the device's being its last):
    /// every input, while some track takes one. The audio callback calls it; it neither locks nor allocates.
    pub fn push(&self, from: u32, frames: usize, at: impl Fn(usize, usize) -> f32) {
        if self.on() && self.from.load(Ordering::Relaxed) == from {
            self.block.fetch_max(frames, Ordering::Relaxed);
            self.ring.push_with(frames * LIVE_INPUTS, |k| at(k % LIVE_INPUTS, k / LIVE_INPUTS));
        }
    }

    /// Takes stream `from`'s inputs (`None`: no track takes one).
    fn set(&self, from: Option<u32>) {
        if let Some(f) = from {
            self.from.store(f, Ordering::Relaxed);
        }
        self.block.store(0, Ordering::Relaxed);
        self.on.store(from.is_some(), Ordering::Release);
    }

    /// A tick's `frames` frames of every input into `out`: silence until `PRIME` frames wait, and when they ran out.
    fn read(&self, frames: usize, out: &mut Vec<f32>) {
        out.clear();
        if let Ok(_one) = self.reader.try_lock() {
            // What it keeps waiting grows with the device's blocks (`block`), so one big block never skips the rest.
            let prime = PRIME + self.block.load(Ordering::Relaxed);
            let most = prime + (MOST - PRIME);
            let waiting = self.ring.len() / LIVE_INPUTS;
            if waiting > most {
                self.ring.take((waiting - prime) * LIVE_INPUTS, out, false);
            }
            let waiting = self.ring.len() / LIVE_INPUTS;
            if !self.primed.load(Ordering::Relaxed) && waiting >= prime.max(PRIME + frames) {
                self.primed.store(true, Ordering::Relaxed);
            }
            if self.primed.load(Ordering::Relaxed) {
                if waiting < frames {
                    self.primed.store(false, Ordering::Relaxed);
                }
                self.ring.take(frames * LIVE_INPUTS, out, true);
            }
        }
        out.resize(frames * LIVE_INPUTS, 0.0);
    }
}

/// The monitor as the engines that play read it (`os_engine::Engine::set_live_input`).
pub fn live_input() -> Arc<dyn os_engine::LiveInput> {
    struct Live;
    impl os_engine::LiveInput for Live {
        fn read(&self, frames: usize, out: &mut Vec<f32>) {
            monitor().read(frames, out);
        }
    }
    Arc::new(Live)
}

/// An input to record from: a device (the ASIO driver, or a system input device) and its first channel, alone or as a
/// pair with the next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    pub device: String,
    pub asio: bool,
    pub first: u16,
    pub stereo: bool,
}

impl Input {
    /// "In 1 - In 2", "In 3" (spec §2 The button and the menu).
    pub fn label(&self) -> String {
        let n = self.first + 1;
        if self.stereo { format!("In {n} - In {}", n + 1) } else { format!("In {n}") }
    }

    /// As kept in `settings.txt` (`rec_input`).
    pub fn key(&self) -> String {
        format!("{}|{}|{}|{}", if self.asio { "asio" } else { "system" }, self.first, if self.stereo { "stereo" } else { "mono" }, self.device)
    }

    pub fn parse(key: &str) -> Option<Input> {
        let mut it = key.splitn(4, '|');
        let asio = it.next()? == "asio";
        let first = it.next()?.parse().ok()?;
        let stereo = it.next()? == "stereo";
        Some(Input { device: it.next()?.to_string(), asio, first, stereo })
    }

    /// The inputs of a device with `channels` inputs: its pairs, then each channel.
    pub fn of(device: &str, asio: bool, channels: u16) -> Vec<Input> {
        let d = |first, stereo| Input { device: device.to_string(), asio, first, stereo };
        let pairs = (0..channels / 2).map(|p| d(2 * p, true));
        pairs.chain((0..channels).map(|c| d(c, false))).collect()
    }
}

/// The input picked last (`rec_input`).
pub fn last_input() -> Option<Input> {
    crate::settings_file::get("rec_input").as_deref().and_then(Input::parse)
}

pub fn set_last_input(i: &Input) {
    crate::settings_file::write("rec_input", &i.key());
}

/// The devices to record from, with their input count: the ASIO driver when the output is one (its inputs once it has
/// opened; until then two are assumed and checked at the start), else the system's input devices.
pub fn devices() -> Vec<(String, bool, u16)> {
    let cfg = crate::audio_cfg::AudioCfg::current();
    if let Some(name) = cfg.asio_driver() {
        let opened = crate::device::last().filter(|d| d.host == crate::device::ASIO && d.name == name).map(|d| d.inputs);
        return vec![(name.to_string(), true, opened.unwrap_or(2))];
    }
    system_inputs()
}

/// The system's input devices and their channels (on Linux the sound server's, or ALSA's in exclusive mode, as the
/// output).
fn system_inputs() -> Vec<(String, bool, u16)> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = crate::audio_cfg::host(&crate::audio_cfg::AudioCfg::current());
    let Ok(list) = host.input_devices() else { return Vec::new() };
    list.filter(crate::audio_cfg::listed).filter_map(|d| {
        let c = d.default_input_config().ok()?;
        Some((crate::audio_cfg::device_name(&d), false, c.channels()))
    })
    .collect()
}

/// A system input device's stream, feeding the capture and the monitor (when they take from it) while it is held.
pub struct InputStream {
    /// Taken when dropped: an ALSA device is let go of before the sound server is given it back.
    stream: Option<cpal::Stream>,
    alsa: bool,
    pub rate: u32,
    /// What it pushes as (`Capture::start`, `Monitor`).
    pub source: u32,
}

impl Drop for InputStream {
    fn drop(&mut self) {
        drop(self.stream.take());
        if self.alsa {
            crate::device::heal_sound_server();
        }
    }
}

/// Opens system input device `name` in shared mode at its own rate, pushing into `capture` and `monitor`.
pub fn open_input(name: &str) -> Result<InputStream, String> {
    use cpal::traits::{HostTrait, StreamTrait};
    let host = crate::audio_cfg::host(&crate::audio_cfg::AudioCfg::current());
    let device = crate::audio_cfg::find(host.input_devices().map_err(|e| e.to_string())?, name).ok_or_else(|| format!("{name} is not there"))?;
    let (stream, rate, source) = open_device(&device)?;
    stream.play().map_err(|e| e.to_string())?;
    let alsa = host.id().name() == "ALSA";
    Ok(InputStream { stream: Some(stream), alsa, rate, source })
}

/// `open_input` for the device named `name`, else (it is gone: another driver chosen, unplugged) the system's default
/// input device; returns it and the name it opened.
pub fn open_input_or_default(name: &str) -> Result<(InputStream, String), String> {
    use cpal::traits::HostTrait;
    match open_input(name) {
        Ok(s) => Ok((s, name.to_string())),
        Err(e) => {
            let host = crate::audio_cfg::host(&crate::audio_cfg::AudioCfg::current());
            let other = host.default_input_device().map(|d| crate::audio_cfg::device_name(&d)).filter(|d| d != name).ok_or(e)?;
            Ok((open_input(&other)?, other))
        }
    }
}

fn open_device(device: &cpal::Device) -> Result<(cpal::Stream, u32, u32), String> {
    use cpal::traits::DeviceTrait;
    use cpal::SampleFormat;
    let supported = device.default_input_config().map_err(|e| e.to_string())?;
    let format = supported.sample_format();
    let range = supported.buffer_size().clone();
    let mut config: cpal::StreamConfig = supported.into();
    let source = next_source();
    let open = |config: &cpal::StreamConfig| match format {
        SampleFormat::F32 => build::<f32>(device, config, source),
        SampleFormat::F64 => build::<f64>(device, config, source),
        SampleFormat::I16 => build::<i16>(device, config, source),
        SampleFormat::I24 => build::<cpal::I24>(device, config, source),
        SampleFormat::I32 => build::<i32>(device, config, source),
        SampleFormat::U16 => build::<u16>(device, config, source),
        other => Err(format!("unsupported input sample format {other:?}")),
    };
    // Small blocks, so a track's input is heard soon and steadily: a sound server's default can be thousands of
    // frames (8192 seen with PulseAudio) at a time. A device that refuses opens at its own size.
    let small = match range {
        cpal::SupportedBufferSize::Range { min, max } => INPUT_BLOCK.clamp(min, max),
        cpal::SupportedBufferSize::Unknown => INPUT_BLOCK,
    };
    let default = config.buffer_size;
    config.buffer_size = cpal::BufferSize::Fixed(small);
    let stream = match open(&config) {
        Ok(s) => s,
        Err(_) => {
            config.buffer_size = default;
            open(&config)?
        }
    };
    Ok((stream, config.sample_rate, source))
}

/// The block an input stream asks its device for (frames): about 10 ms at 48 kHz.
const INPUT_BLOCK: u32 = 512;

fn build<T>(device: &cpal::Device, config: &cpal::StreamConfig, source: u32) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    use cpal::traits::DeviceTrait;
    let channels = usize::from(config.channels).max(1);
    device
        .build_input_stream::<T, _, _>(
            config.clone(),
            move |data: &[T], _| {
                // An input past the device's channels is its last one.
                let at = |c: usize, f: usize| data[f * channels + c.min(channels - 1)].to_sample::<f32>();
                capture().push(source, data.len() / channels, at);
                monitor().push(source, data.len() / channels, at);
            },
            |e| eprintln!("audio input stream: {e}"),
            None,
        )
        .map_err(|e| e.to_string())
}

/// The tracks' inputs come from `want` (a device's name; `None`: no track takes one): the ASIO driver's callback when
/// the output is one, else system input stream `open` (opened here, or kept when it is already that device's).
/// Returns an error once, when the device does not open.
pub fn monitor_inputs(want: Option<&str>, open: &mut Option<(String, InputStream)>) -> Result<(), String> {
    // Tests open no device: the inputs are pushed as the driver's.
    let asio = cfg!(test) || crate::audio_cfg::AudioCfg::current().asio_driver().is_some();
    match want {
        None => {
            monitor().set(None);
            *open = None;
        }
        Some(_) if asio => {
            *open = None;
            monitor().set(Some(ASIO_SOURCE));
        }
        Some(name) => {
            if open.as_ref().is_some_and(|(n, _)| n == name) {
                return Ok(());
            }
            *open = None;
            monitor().set(None);
            let (s, _) = open_input_or_default(name)?;
            monitor().set(Some(s.source));
            *open = Some((name.to_string(), s));
        }
    }
    Ok(())
}

/// The take's file name: "<project>_<YYYY-MM-DD HH-MM-SS>_REC.wav" (spec §2 The take).
pub fn file_name(project: &str, now: chrono::NaiveDateTime) -> String {
    format!("{project}_{}_REC.wav", now.format("%Y-%m-%d %H-%M-%S"))
}

/// The writer: drains the capture into `<file>.part` until finished, then makes the WAV.
pub struct Recorder {
    stop: Arc<AtomicBool>,
    /// The first frame of the take (frames before it are left out), set before `finish`.
    head: Arc<AtomicI64>,
    /// The WAV's rate, set before `finish` when it was not known at the start (an ASIO driver's).
    rate: Arc<AtomicU32>,
    thread: Option<std::thread::JoinHandle<Result<(PathBuf, u64), String>>>,
}

impl Recorder {
    /// Starts writing what the capture takes in into `file` (a WAV at `rate` once finished).
    pub fn start(file: PathBuf, rate: u32) -> Result<Recorder, String> {
        if let Some(d) = file.parent() {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        let part = file.with_extension("wav.part");
        let raw = std::fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?;
        let stop = Arc::new(AtomicBool::new(false));
        let head = Arc::new(AtomicI64::new(0));
        let rate = Arc::new(AtomicU32::new(rate));
        let (s, h, r) = (stop.clone(), head.clone(), rate.clone());
        let thread = std::thread::Builder::new()
            .name("audio recorder".into())
            .spawn(move || {
                let mut w = BufWriter::new(raw);
                let mut buf = Vec::with_capacity(1 << 16);
                let mut samples = 0u64;
                loop {
                    let last = s.load(Ordering::Acquire);
                    buf.clear();
                    capture().drain(&mut buf);
                    for x in &buf {
                        w.write_all(&x.to_le_bytes()).map_err(|e| e.to_string())?;
                    }
                    samples += buf.len() as u64;
                    if last {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                let raw = w.into_inner().map_err(|e| e.to_string())?;
                drop(raw);
                let frames = samples / 2;
                let head = h.load(Ordering::Acquire);
                let n = to_wav(&part, &file, r.load(Ordering::Acquire), head, frames).map_err(|e| e.to_string());
                let _ = std::fs::remove_file(&part);
                Ok((file, n?))
            })
            .map_err(|e| e.to_string())?;
        Ok(Recorder { stop, head, rate, thread: Some(thread) })
    }

    /// The take starts at frame `head` of what came in (negative: that much silence first).
    pub fn set_head(&self, head: i64) {
        self.head.store(head, Ordering::Release);
    }

    pub fn set_rate(&self, rate: u32) {
        self.rate.store(rate, Ordering::Release);
    }

    /// Stops taking in; the WAV is made on the writer thread (`done`, then `join`).
    pub fn finish(&self) {
        self.stop.store(true, Ordering::Release);
    }

    pub fn done(&self) -> bool {
        self.thread.as_ref().is_none_or(|t| t.is_finished())
    }

    /// The WAV and its frames.
    pub fn join(mut self) -> Result<(PathBuf, u64), String> {
        self.thread.take().ok_or("already joined")?.join().map_err(|_| "the recorder stopped".to_string())?
    }
}

/// Makes WAV `wav` (32-bit float stereo at `rate`) of the raw frames in `part` from frame `head` on; returns its frames.
fn to_wav(part: &Path, wav: &Path, rate: u32, head: i64, frames: u64) -> std::io::Result<u64> {
    let skip = head.max(0) as u64;
    let pad = (-head).max(0) as u64;
    let n = frames.saturating_sub(skip) + pad;
    let mut out = BufWriter::new(std::fs::File::create(wav)?);
    os_engine::wav::write_f32_stereo_header(&mut out, rate, n)?;
    out.write_all(&vec![0u8; (pad * 8) as usize])?;
    let mut r = std::fs::File::open(part)?;
    r.seek(SeekFrom::Start(skip.min(frames) * 8))?;
    std::io::copy(&mut r.take(frames.saturating_sub(skip) * 8), &mut out)?;
    out.flush()?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_keeps_order_and_drops_what_has_no_room() {
        let r = Ring::new(4);
        assert_eq!(r.push_with(3, |k| k as f32), 3);
        let mut out = Vec::new();
        r.pop_into(&mut out);
        assert_eq!(out, [0.0, 1.0, 2.0]);
        assert_eq!(r.push_with(6, |k| 10.0 + k as f32), 4, "four slots");
        out.clear();
        r.pop_into(&mut out);
        assert_eq!(out, [10.0, 11.0, 12.0, 13.0], "wrapped around");
    }

    #[test]
    fn inputs_their_labels_and_keys() {
        let v = Input::of("Driver", true, 3);
        let labels: Vec<String> = v.iter().map(Input::label).collect();
        assert_eq!(labels, ["In 1 - In 2", "In 1", "In 2", "In 3"]);
        assert_eq!(Input::parse(&v[0].key()), Some(v[0].clone()));
        assert_eq!(Input::parse(&v[3].key()), Some(v[3].clone()));
        let t = chrono::NaiveDate::from_ymd_opt(2026, 10, 19).unwrap().and_hms_opt(21, 48, 43).unwrap();
        assert_eq!(file_name("untitled", t), "untitled_2026-10-19 21-48-43_REC.wav");
    }

    /// The WAV leaves out the frames before the head, or starts with silence for a negative one.
    #[test]
    fn a_wav_from_the_head_on() {
        let dir = std::env::temp_dir().join(format!("lemon_capture_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join("t.wav.part");
        let raw: Vec<u8> = (0..8).flat_map(|i| (i as f32).to_le_bytes()).collect();
        std::fs::write(&part, raw).unwrap();
        let wav = dir.join("t.wav");
        assert_eq!(to_wav(&part, &wav, 48000, 1, 4).unwrap(), 3);
        let b = std::fs::read(&wav).unwrap();
        let data: Vec<f32> = b[44..].chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
        assert_eq!(data, [2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
        assert_eq!(to_wav(&part, &wav, 48000, -1, 4).unwrap(), 5);
        let b = std::fs::read(&wav).unwrap();
        assert_eq!(b.len(), 44 + 5 * 8);
        assert_eq!(&b[44..52], &[0u8; 8]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A real input device takes in (needs one: `cargo test -- --ignored`).
    #[test]
    #[ignore]
    fn a_system_input_takes_in() {
        let (name, ..) = system_inputs().into_iter().next().expect("an input device");
        capture().start(ASIO_SOURCE, 0, true);
        let stream = open_input(&name).expect("opens");
        let rate = stream.rate;
        std::thread::sleep(std::time::Duration::from_millis(500));
        drop(stream);
        capture().stop();
        let frames = capture().frames();
        assert!(frames > 0, "{name}: nothing came in");
        eprintln!("{name}: {frames} frames in 0.5 s at {rate} Hz");
    }

    /// The monitor hands out a tick of every input once enough waits, silence before that and when it runs dry, and
    /// skips back when too much waits; only its stream goes in.
    #[test]
    fn the_monitor_keeps_a_little_waiting() {
        let m = Monitor { ring: Ring::new(MONITOR_RING), on: AtomicBool::new(false), from: AtomicU32::new(ASIO_SOURCE), primed: AtomicBool::new(false), reader: Mutex::new(()), block: std::sync::atomic::AtomicUsize::new(0) };
        let at = |c: usize, f: usize| (f * 100 + c) as f32;
        // A low-latency device: 64-frame blocks.
        let push = |m: &Monitor, n: usize| (0..n / 64).for_each(|_| m.push(3, 64, at));
        m.push(ASIO_SOURCE, 64, at);
        assert_eq!(m.ring.len(), 0, "off");
        m.set(Some(3));
        m.push(ASIO_SOURCE, 64, at);
        assert_eq!(m.ring.len(), 0, "another stream's");
        push(&m, PRIME);
        let mut out = Vec::new();
        m.read(100, &mut out);
        assert_eq!(out.len(), 100 * LIVE_INPUTS);
        assert!(out.iter().all(|&x| x == 0.0), "not enough waiting yet");
        push(&m, 192);
        m.read(100, &mut out);
        assert_eq!(out[LIVE_INPUTS + 2], 102.0, "the second frame's third input");
        assert_eq!(m.ring.len(), (PRIME + 92) * LIVE_INPUTS);
        push(&m, MOST);
        m.read(10, &mut out);
        assert_eq!(m.ring.len(), (PRIME + 64 - 10) * LIVE_INPUTS, "skipped back to a block more than PRIME");
        m.set(None);
        assert!(!m.on());
    }

    /// A device that hands its input over in big blocks (8192 frames, as PulseAudio did): the monitor keeps a block
    /// waiting, so a tick's input sounds and goes on sounding until the next block comes, never skipped to silence.
    #[test]
    fn the_monitor_plays_a_device_with_big_blocks() {
        let m = Monitor { ring: Ring::new(MONITOR_RING), on: AtomicBool::new(false), from: AtomicU32::new(ASIO_SOURCE), primed: AtomicBool::new(false), reader: Mutex::new(()), block: std::sync::atomic::AtomicUsize::new(0) };
        let one = |_: usize, _: usize| 0.5f32;
        m.set(Some(3));
        m.push(3, 8192, one);
        let mut out = Vec::new();
        m.read(231, &mut out);
        assert!(out.iter().all(|&x| x == 0.0), "waits for a block on top of PRIME");
        m.push(3, 8192, one);
        // The reads of a block's time (8192 frames, 35 ticks of 231) all sound.
        for k in 0..35 {
            m.read(231, &mut out);
            assert!(out.iter().all(|&x| x == 0.5), "tick {k} sounds");
        }
    }

    /// Stereo takes a pair; one input goes to both sides.
    #[test]
    fn a_block_goes_in_as_stereo() {
        let c = Capture {
            ring: Ring::new(64),
            on: AtomicBool::new(false),
            from: AtomicU32::new(ASIO_SOURCE),
            first: AtomicU32::new(0),
            stereo: AtomicBool::new(true),
            frames: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
        };
        c.push(ASIO_SOURCE, 2, |ch, f| (ch * 10 + f) as f32);
        assert_eq!(c.frames(), 0, "off");
        c.start(ASIO_SOURCE, 1, true);
        c.push(ASIO_SOURCE, 2, |ch, f| (ch * 10 + f) as f32);
        c.start(7, 2, false);
        c.push(ASIO_SOURCE, 3, |ch, f| (ch * 10 + f) as f32);
        c.push(7, 1, |ch, f| (ch * 10 + f) as f32);
        let mut out = Vec::new();
        c.drain(&mut out);
        assert_eq!(out, [20.0, 20.0], "start empties what came before; only its stream goes in");
        assert_eq!(c.frames(), 1);
    }
}
