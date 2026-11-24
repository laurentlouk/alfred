//! Speech out, in-process: a stream on the default output device, at the system volume. Sentences are
//! queued as they render and play back to back.
//!
//! Opening the device is slow (about 0.5 s for Bluetooth headphones, on every open), so [`Speakers`] keeps
//! the stream open while the user talks with Alfred: [`Speakers::warm`] opens it as soon as a request
//! starts, and it closes after [`IDLE`] of quiet. The audio callback only copies queued samples (silence
//! when there are none); resampling happens on the caller's thread, and a thread of its own holds the
//! stream.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use sherpa_onnx::LinearResampler;

/// How long the stream stays open with nothing to play.
pub const IDLE: Duration = Duration::from_secs(30);
/// The longest a sentence waits for the device to open.
const OPEN_WAIT: Duration = Duration::from_secs(3);

/// What the stream's callback and the caller share: mono samples at the device's rate.
#[derive(Default)]
pub struct Queue {
    samples: Mutex<VecDeque<f32>>,
    /// Samples ever queued, and samples the device has taken: their difference is what's left to play.
    queued: AtomicU64,
    played: AtomicU64,
    rate: AtomicU64,
}

impl Queue {
    /// Fill an interleaved output buffer of `channels` channels from the queue, silence where it runs out.
    pub fn fill<T: SizedSample + FromSample<f32>>(&self, out: &mut [T], channels: usize) {
        let channels = channels.max(1);
        let silence = T::from_sample(0.0f32);
        let Ok(mut q) = self.samples.lock() else {
            out.fill(silence);
            return;
        };
        let mut taken = 0u64;
        for frame in out.chunks_mut(channels) {
            let s = q.pop_front().map(|x| {
                taken += 1;
                T::from_sample(x)
            });
            frame.fill(s.unwrap_or(silence));
        }
        self.played.fetch_add(taken, Ordering::Relaxed);
    }

    /// Queue samples; returns the second, from the stream's start, at which they begin.
    fn push(&self, samples: &[f32]) -> f32 {
        let Ok(mut q) = self.samples.lock() else { return self.position() };
        let starts = self.queued.load(Ordering::Relaxed) as f32 / self.rate().max(1) as f32;
        q.extend(samples);
        self.queued.fetch_add(samples.len() as u64, Ordering::Relaxed);
        starts
    }

    /// Drop what's queued: silence from the next buffer on.
    fn clear(&self) {
        if let Ok(mut q) = self.samples.lock() {
            q.clear();
            self.queued.store(self.played.load(Ordering::Relaxed), Ordering::Relaxed);
        }
    }

    fn rate(&self) -> u64 {
        self.rate.load(Ordering::Relaxed)
    }

    /// Seconds of audio played since the stream opened.
    pub fn position(&self) -> f32 {
        self.played.load(Ordering::Relaxed) as f32 / self.rate().max(1) as f32
    }

    fn drained(&self) -> bool {
        self.played.load(Ordering::Relaxed) >= self.queued.load(Ordering::Relaxed)
    }
}

/// An open (or opening) stream.
struct Stream {
    queue: Arc<Queue>,
    /// The device's rate once it's open, or why it couldn't open.
    ready: Arc<OnceLock<Result<u32, String>>>,
    /// Resamples from a source rate to the device's.
    resampler: Option<(u32, LinearResampler)>,
    close: Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl Stream {
    fn open() -> Stream {
        let queue = Arc::new(Queue::default());
        let ready = Arc::new(OnceLock::new());
        let (close, closing) = channel::<()>();
        let (q, r) = (queue.clone(), ready.clone());
        let thread = std::thread::Builder::new()
            .name("alfred-speakers".into())
            .spawn(move || match start_stream(q.clone()) {
                Ok((stream, rate)) => {
                    q.rate.store(rate as u64, Ordering::Relaxed);
                    r.set(Ok(rate)).ok();
                    // Hold the stream until told to close.
                    closing.recv().ok();
                    stream.pause().ok();
                }
                Err(e) => {
                    r.set(Err(e)).ok();
                }
            })
            .ok();
        Stream { queue, ready, resampler: None, close, thread }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        self.close.send(()).ok();
        if let Some(t) = self.thread.take() {
            t.join().ok();
        }
    }
}

/// The speakers, opened on demand and kept open while Alfred is in use.
pub struct Speakers {
    stream: Mutex<Option<Stream>>,
    last_use: Mutex<Instant>,
}

impl Speakers {
    /// The speakers, closed until first used. A thread closes them after [`IDLE`] of quiet.
    pub fn new() -> Arc<Speakers> {
        let me = Arc::new(Speakers { stream: Mutex::new(None), last_use: Mutex::new(Instant::now()) });
        let weak: Weak<Speakers> = Arc::downgrade(&me);
        std::thread::Builder::new()
            .name("alfred-speakers-idle".into())
            .spawn(move || {
                while let Some(s) = weak.upgrade() {
                    s.close_if_idle(IDLE);
                    drop(s);
                    std::thread::sleep(Duration::from_secs(1));
                }
            })
            .ok();
        me
    }

    fn touch(&self) {
        if let Ok(mut t) = self.last_use.lock() {
            *t = Instant::now();
        }
    }

    /// Start opening the device now, without waiting, so speech can start the moment it's ready.
    pub fn warm(&self) {
        self.touch();
        if let Ok(mut s) = self.stream.lock() {
            let failed = s.as_ref().is_some_and(|st| matches!(st.ready.get(), Some(Err(_))));
            if s.is_none() || failed {
                *s = Some(Stream::open());
            }
        }
    }

    /// Queue `samples` (mono, at `rate` Hz) after what's already queued, opening the device if needed.
    /// Returns the second, on [`Speakers::position`]'s clock, at which they begin to play.
    pub fn push(&self, samples: &[f32], rate: u32) -> Result<f32, String> {
        self.warm();
        // Wait for the device without holding the lock, so stopping is never held up.
        let ready = self.stream.lock().map_err(|_| "speakers unavailable")?.as_ref().map(|s| s.ready.clone());
        let ready = ready.ok_or("speakers unavailable")?;
        let started = Instant::now();
        let device_rate = loop {
            if let Some(r) = ready.get() {
                break r.clone()?;
            }
            if started.elapsed() > OPEN_WAIT {
                return Err("the speakers didn't open in time".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        let mut guard = self.stream.lock().map_err(|_| "speakers unavailable")?;
        let st = guard.as_mut().ok_or("speakers unavailable")?;
        if rate == device_rate {
            st.resampler = None;
            return Ok(st.queue.push(samples));
        }
        if st.resampler.as_ref().map(|(from, _)| *from) != Some(rate) {
            let r = LinearResampler::create(rate as i32, device_rate as i32).ok_or("couldn't set up the resampler")?;
            st.resampler = Some((rate, r));
        }
        let converted = st.resampler.as_ref().map(|(_, r)| r.resample(samples, false)).unwrap_or_default();
        Ok(st.queue.push(&converted))
    }

    /// The reply is complete: queue what the resampler still holds (its last few milliseconds), and start
    /// the next reply's resampling afresh.
    pub fn finish(&self) {
        if let Ok(mut s) = self.stream.lock()
            && let Some(st) = s.as_mut()
            && let Some((_, r)) = st.resampler.take()
        {
            st.queue.push(&r.resample(&[], true));
        }
    }

    /// Seconds played since the stream opened (0 when closed).
    pub fn position(&self) -> f32 {
        self.stream.lock().ok().and_then(|s| s.as_ref().map(|st| st.queue.position())).unwrap_or(0.0)
    }

    /// Nothing is left to play.
    pub fn drained(&self) -> bool {
        self.stream.lock().ok().and_then(|s| s.as_ref().map(|st| st.queue.drained())).unwrap_or(true)
    }

    /// Silence now: drop everything queued, and what the resampler holds. The stream stays open for what
    /// comes next.
    pub fn stop(&self) {
        if let Ok(mut s) = self.stream.lock()
            && let Some(st) = s.as_mut()
        {
            st.resampler = None;
            st.queue.clear();
        }
    }

    /// Close the stream when nothing has been queued for `idle` and it has played everything.
    pub fn close_if_idle(&self, idle: Duration) {
        let quiet = self.last_use.lock().map(|t| t.elapsed() >= idle).unwrap_or(false);
        if !quiet {
            return;
        }
        if let Ok(mut s) = self.stream.lock()
            && s.as_ref().is_some_and(|st| st.queue.drained())
        {
            s.take();
        }
    }

    /// The stream is open (or opening).
    pub fn is_open(&self) -> bool {
        self.stream.lock().map(|s| s.is_some()).unwrap_or(false)
    }
}

/// Open the default output device at its own rate and start a stream that plays `queue`.
fn start_stream(queue: Arc<Queue>) -> Result<(cpal::Stream, u32), String> {
    let device = cpal::default_host().default_output_device().ok_or("no speakers or headphones found")?;
    let supported = device.default_output_config().map_err(|e| format!("speakers: {e}"))?;
    let channels = supported.channels().max(1) as usize;
    let rate = supported.sample_rate();
    let config = supported.config();
    let stream = match supported.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, config, channels, queue),
        SampleFormat::I16 => build::<i16>(&device, config, channels, queue),
        SampleFormat::I32 => build::<i32>(&device, config, channels, queue),
        SampleFormat::U16 => build::<u16>(&device, config, channels, queue),
        SampleFormat::F64 => build::<f64>(&device, config, channels, queue),
        other => return Err(format!("the speakers' sample format {other} isn't supported")),
    }
    .map_err(|e| format!("speakers: {e}"))?;
    stream.play().map_err(|e| format!("speakers: {e}"))?;
    Ok((stream, rate))
}

fn build<T>(device: &cpal::Device, config: cpal::StreamConfig, channels: usize, queue: Arc<Queue>) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample + FromSample<f32>,
{
    device.build_output_stream::<T, _, _>(
        config,
        move |out: &mut [T], _: &cpal::OutputCallbackInfo| queue.fill(out, channels),
        |_e: cpal::Error| {},
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue_at(rate: u64) -> Queue {
        let q = Queue::default();
        q.rate.store(rate, Ordering::Relaxed);
        q
    }

    #[test]
    fn the_callback_plays_queued_samples_on_every_channel_then_silence() {
        let q = queue_at(4);
        assert_eq!(q.push(&[0.5, -0.5]), 0.0);
        assert_eq!(q.push(&[0.25]), 0.5, "the next sentence starts where the first ends");
        let mut out = [9.0f32; 8];
        q.fill(&mut out, 2);
        assert_eq!(out, [0.5, 0.5, -0.5, -0.5, 0.25, 0.25, 0.0, 0.0]);
        assert!(q.drained());
        assert_eq!(q.position(), 0.75, "three samples at 4 Hz");
    }

    #[test]
    fn clearing_silences_at_once_and_the_queue_takes_more_after() {
        let q = queue_at(4);
        q.push(&[0.5; 8]);
        q.clear();
        assert!(q.drained());
        let mut out = [1i16; 4];
        q.fill(&mut out, 1);
        assert_eq!(out, [0; 4]);
        assert_eq!(q.push(&[0.5]), 0.0, "what comes next plays right away");
    }

    /// Opens the real speakers: how long until they take the first samples, cold and then warm.
    /// Run with `cargo test --release -p alfred-voice -- --ignored --nocapture measure_speakers`.
    #[test]
    #[ignore]
    fn measure_speakers() {
        let speakers = Speakers::new();
        for round in ["cold", "warm", "warm"] {
            let started = Instant::now();
            let at = speakers.push(&vec![0.0; 2400], 24_000).unwrap();
            while speakers.position() <= at {
                std::thread::sleep(Duration::from_millis(1));
            }
            eprintln!("speakers ({round}): first samples taken after {:.0} ms", started.elapsed().as_secs_f32() * 1000.0);
            while !speakers.drained() {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        let started = Instant::now();
        let at = speakers.push(&vec![0.0; 24_000 * 2], 24_000).unwrap();
        while !speakers.drained() {
            std::thread::sleep(Duration::from_millis(1));
        }
        eprintln!(
            "speakers: 2 s queued at {at:.2} s played out in {:.2} s (position {:.2} s)",
            started.elapsed().as_secs_f32(),
            speakers.position()
        );
        let closing = Instant::now();
        drop(speakers);
        eprintln!("speakers: closed in {:.0} ms", closing.elapsed().as_secs_f32() * 1000.0);
    }
}
