//! Microphone capture: the default input device, downmixed to mono and resampled to 16 kHz.
//!
//! The audio callback only converts and queues samples; all decoding happens on the reader's thread.
//! Capture runs from [`Mic::open`] until the [`Mic`] is closed or dropped, and macOS shows its
//! microphone indicator for exactly that span.

use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TryRecvError, sync_channel};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{ErrorKind, FromSample, Sample, SampleFormat, SizedSample};
use sherpa_onnx::LinearResampler;

use crate::vad::SAMPLE_RATE;

/// Callback buffers the queue holds before it drops audio: about 5 s at 10 ms per buffer. The reader
/// falls this far behind only while it transcribes, and catches up right after.
const QUEUE: usize = 512;

/// The default microphone, capturing.
pub struct Mic {
    stream: cpal::Stream,
    audio: Receiver<Vec<f32>>,
    errors: Receiver<String>,
    resampler: Option<LinearResampler>,
    /// The device's own rate.
    rate: u32,
}

impl Mic {
    /// Open the default input device and start capturing. On macOS the first call asks the user for
    /// microphone access (the app's `NSMicrophoneUsageDescription`).
    pub fn open() -> Result<Mic, String> {
        let host = cpal::default_host();
        let device = host.default_input_device().ok_or("no microphone found")?;
        let supported = device.default_input_config().map_err(|e| format!("microphone: {e}"))?;
        let channels = supported.channels().max(1) as usize;
        let rate = supported.sample_rate();
        let config = supported.config();
        let (tx, audio) = sync_channel(QUEUE);
        let (etx, errors) = sync_channel(8);
        let stream = match supported.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, config, channels, tx, etx),
            SampleFormat::I16 => build::<i16>(&device, config, channels, tx, etx),
            SampleFormat::I32 => build::<i32>(&device, config, channels, tx, etx),
            SampleFormat::U16 => build::<u16>(&device, config, channels, tx, etx),
            SampleFormat::I8 => build::<i8>(&device, config, channels, tx, etx),
            SampleFormat::U8 => build::<u8>(&device, config, channels, tx, etx),
            SampleFormat::F64 => build::<f64>(&device, config, channels, tx, etx),
            other => return Err(format!("the microphone's sample format {other} isn't supported")),
        }
        .map_err(|e| format!("microphone: {e}"))?;
        stream.play().map_err(|e| format!("microphone: {e}"))?;
        let resampler = if rate == SAMPLE_RATE {
            None
        } else {
            Some(LinearResampler::create(rate as i32, SAMPLE_RATE as i32).ok_or("couldn't set up the resampler")?)
        };
        Ok(Mic { stream, audio, errors, resampler, rate })
    }

    /// The next audio at 16 kHz mono, waiting at most `wait`: `Ok(None)` when none came. An error means
    /// the stream is gone (the device was unplugged or changed) and the mic must be opened again.
    pub fn read(&mut self, wait: Duration) -> Result<Option<Vec<f32>>, String> {
        match self.errors.try_recv() {
            Ok(e) => return Err(e),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => return Err("the microphone stopped".into()),
        }
        let mut raw = match self.audio.recv_timeout(wait) {
            Ok(chunk) => chunk,
            Err(RecvTimeoutError::Timeout) => return Ok(None),
            Err(RecvTimeoutError::Disconnected) => return Err("the microphone stopped".into()),
        };
        // Take whatever else is queued, so one read catches up after a slow step.
        while let Ok(more) = self.audio.try_recv() {
            raw.extend_from_slice(&more);
        }
        Ok(Some(match &self.resampler {
            Some(r) => r.resample(&raw, false),
            None => raw,
        }))
    }

    /// The device's own sample rate, before resampling.
    pub fn device_rate(&self) -> u32 {
        self.rate
    }

    /// Stop capturing. Dropping the `Mic` does the same.
    pub fn close(self) {
        self.stream.pause().ok();
    }
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    audio: SyncSender<Vec<f32>>,
    errors: SyncSender<String>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    device.build_input_stream::<T, _, _>(
        config,
        // A full queue drops audio rather than block the audio thread.
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            let _ = audio.try_send(downmix(data, channels));
        },
        move |e: cpal::Error| {
            if matches!(e.kind(), ErrorKind::DeviceNotAvailable | ErrorKind::DeviceChanged | ErrorKind::StreamInvalidated) {
                let _ = errors.try_send(format!("microphone: {e}"));
            }
        },
        None,
    )
}

/// Interleaved frames of `channels` samples, averaged to mono floats.
pub fn downmix<T>(data: &[T], channels: usize) -> Vec<f32>
where
    T: Sample,
    f32: FromSample<T>,
{
    let channels = channels.max(1);
    data.chunks_exact(channels).map(|frame| frame.iter().map(|s| s.to_sample::<f32>()).sum::<f32>() / channels as f32).collect()
}

/// `samples` at `from` Hz, resampled to `to` Hz with sherpa-onnx's band-limited resampler.
pub fn resample(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || samples.is_empty() {
        return samples.to_vec();
    }
    match LinearResampler::create(from as i32, to as i32) {
        Some(r) => r.resample(samples, true),
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_and_integer_audio_become_mono_floats() {
        assert_eq!(downmix(&[1.0f32, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
        assert_eq!(downmix(&[0.25f32, -0.25], 1), vec![0.25, -0.25]);
        let m = downmix(&[i16::MAX, i16::MIN, 0, 0], 2);
        assert!(m[0].abs() < 1e-3 && m[1] == 0.0, "{m:?}");
        assert!((downmix(&[16_384i16], 1)[0] - 0.5).abs() < 1e-3);
        // A half frame at the end is left out.
        assert_eq!(downmix(&[0.1f32, 0.1, 0.2], 2).len(), 1);
    }

    #[test]
    fn resampling_keeps_pitch_and_duration() {
        // One second of a 440 Hz tone at 48 kHz.
        let tone: Vec<f32> = (0..48_000).map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5).collect();
        let out = resample(&tone, 48_000, 16_000);
        assert!((out.len() as i32 - 16_000).abs() < 40, "{} samples", out.len());
        // Count rising zero crossings in the middle second's worth: 440 per second.
        let mid = &out[1_000..15_000];
        let rising = mid.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count() as f32 / (14_000.0 / 16_000.0);
        assert!((rising - 440.0).abs() < 3.0, "{rising} Hz");
        let peak = mid.iter().copied().fold(0.0f32, f32::max);
        assert!((peak - 0.5).abs() < 0.03, "peak {peak}");
        assert_eq!(resample(&tone[..10], 16_000, 16_000), tone[..10].to_vec());
    }
}
