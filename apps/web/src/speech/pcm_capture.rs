//! Microphone capture with cpal, doing what the fork's `pcmWorklet.ts` does in the browser:
//! channels mixed to mono, a level for the meter twenty times a second (smoothed so brief
//! peaks soften without delay), audio resampled to 16 kHz and delivered in half-second chunks,
//! a five-minute limit, and a `Stopped` once the last audio is out. Meter-only captures (the
//! microphone test) keep the native-rate audio for local playback instead.
//!
//! One thread owns the cpal stream and the processing; the audio callback only mixes and
//! forwards. Builds with `test-support` can replace the device with injected PCM
//! ([`test_source`]), which goes through the same processing.

use std::sync::mpsc;
use std::time::Duration;

use cpal::traits::{DeviceTrait as _, HostTrait as _, StreamTrait as _};
use t3code_client_runtime::voice_input::speech::SPEECH_SAMPLE_RATE;

use super::pcm_resampler::PcmResampler;

/// Samples per delivered chunk: half a second at 16 kHz.
const CHUNK_SAMPLES: usize = 8_000;
/// Five minutes at 16 kHz.
const LIMIT_SAMPLES: usize = SPEECH_SAMPLE_RATE as usize * 300;

/// What the capture thread reports.
#[derive(Debug)]
pub enum CaptureEvent {
    /// The meter level, 0 to 1.
    Level(f32),
    /// 16 kHz mono samples.
    Pcm(Vec<f32>),
    /// The meter-only capture's audio at the device rate, sent before `Stopped`.
    Recorded { samples: Vec<f32>, rate: u32 },
    /// Everything recorded has been delivered.
    Stopped,
    /// Five minutes reached; recording stopped.
    Limit,
    /// The device reported an error mid-capture.
    Failed(CaptureFailure),
}

/// How a running capture broke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureFailure {
    /// "Microphone processing failed."
    Processing,
    /// "The microphone disconnected."
    Disconnected,
}

/// Why the microphone did not open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// The system refused access.
    PermissionDenied,
    /// No input device, or the chosen one is gone and there is no default.
    NotFound,
    Failed(String),
}

/// What to capture.
#[derive(Debug, Clone, Default)]
pub struct CaptureOptions {
    /// The device name; `None` is the system default.
    pub device: Option<String>,
    /// Only the level and the native-rate audio (the microphone test).
    pub meter_only: bool,
}

enum Message {
    Audio(Vec<f32>),
    Start,
    Stop,
    Close,
    StreamError(CaptureFailure),
}

/// An open microphone. Dropping it closes the device.
pub struct PcmCapture {
    messages: mpsc::Sender<Message>,
}

/// The receiving half of a capture's events.
pub type CaptureEvents = futures::channel::mpsc::UnboundedReceiver<CaptureEvent>;

impl PcmCapture {
    /// Opens the microphone. Blocks while the device opens, so call it off the UI thread.
    pub fn open(options: CaptureOptions) -> Result<(Self, CaptureEvents), OpenError> {
        let (messages, receiver) = mpsc::channel();
        let (events, events_rx) = futures::channel::mpsc::unbounded();
        let (opened_tx, opened) = mpsc::channel();
        let feed = messages.clone();
        std::thread::Builder::new()
            .name("t3-pcm-capture".into())
            .spawn(move || {
                let source = match open_source(&options, feed) {
                    Ok(source) => source,
                    Err(error) => {
                        let _ = opened_tx.send(Err(error));
                        return;
                    }
                };
                let rate = source.rate();
                let Some(resampler) = PcmResampler::new(rate) else {
                    let _ = opened_tx.send(Err(OpenError::Failed("Microphone sample rate must be at least 16 kHz.".into())));
                    return;
                };
                let _ = opened_tx.send(Ok(()));
                Processor::new(rate, resampler, options.meter_only, events).run(receiver);
                drop(source);
            })
            .map_err(|error| OpenError::Failed(error.to_string()))?;
        opened.recv().map_err(|_| OpenError::Failed("The capture thread stopped.".into()))??;
        Ok((Self { messages }, events_rx))
    }

    /// Starts keeping audio (`postMessage("start")`).
    pub fn start(&self) {
        let _ = self.messages.send(Message::Start);
    }

    /// Stops keeping audio; `Stopped` follows the last chunk.
    pub fn stop(&self) {
        let _ = self.messages.send(Message::Stop);
    }
}

impl Drop for PcmCapture {
    fn drop(&mut self) {
        let _ = self.messages.send(Message::Close);
    }
}

/// What delivers raw mono audio to the processor.
enum Source {
    Device { _stream: cpal::Stream, rate: u32 },
    #[cfg(feature = "test-support")]
    Injected { rate: u32, _stop: test_source::Feeder },
}

impl Source {
    fn rate(&self) -> u32 {
        match self {
            Self::Device { rate, .. } => *rate,
            #[cfg(feature = "test-support")]
            Self::Injected { rate, .. } => *rate,
        }
    }
}

fn open_source(options: &CaptureOptions, feed: mpsc::Sender<Message>) -> Result<Source, OpenError> {
    #[cfg(feature = "test-support")]
    if let Some(source) = test_source::current() {
        if source.deny {
            return Err(OpenError::PermissionDenied);
        }
        let rate = source.rate;
        return Ok(Source::Injected { rate, _stop: test_source::Feeder::start(source, feed) });
    }
    if super::mac_permission::denied() {
        return Err(OpenError::PermissionDenied);
    }
    let host = cpal::default_host();
    let named = options.device.as_ref().and_then(|name| {
        host.input_devices().ok()?.find(|device| device.name().ok().as_deref() == Some(name.as_str()))
    });
    // A missing chosen device falls back to the default, as `requestMicrophone` does.
    let device = named.or_else(|| host.default_input_device()).ok_or(OpenError::NotFound)?;
    let supported = device.default_input_config().map_err(|error| OpenError::Failed(error.to_string()))?;
    let config: cpal::StreamConfig = supported.config();
    let rate = config.sample_rate.0;
    use cpal::SampleFormat as F;
    let stream = match supported.sample_format() {
        F::F32 => build::<f32>(&device, &config, feed),
        F::F64 => build::<f64>(&device, &config, feed),
        F::I16 => build::<i16>(&device, &config, feed),
        F::I32 => build::<i32>(&device, &config, feed),
        F::I8 => build::<i8>(&device, &config, feed),
        F::U8 => build::<u8>(&device, &config, feed),
        F::U16 => build::<u16>(&device, &config, feed),
        F::U32 => build::<u32>(&device, &config, feed),
        other => return Err(OpenError::Failed(format!("unsupported sample format {other}"))),
    }
    .map_err(|error| OpenError::Failed(error.to_string()))?;
    stream.play().map_err(|error| OpenError::Failed(error.to_string()))?;
    Ok(Source::Device { _stream: stream, rate })
}

fn build<T>(device: &cpal::Device, config: &cpal::StreamConfig, feed: mpsc::Sender<Message>) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = usize::from(config.channels.max(1));
    let errors = feed.clone();
    device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            let mono = data
                .chunks(channels)
                .map(|frame| frame.iter().map(|sample| <f32 as cpal::FromSample<T>>::from_sample_(*sample)).sum::<f32>() / channels as f32)
                .collect();
            let _ = feed.send(Message::Audio(mono));
        },
        move |error| {
            let failure = match error {
                cpal::StreamError::DeviceNotAvailable => CaptureFailure::Disconnected,
                cpal::StreamError::BackendSpecific { .. } => CaptureFailure::Processing,
            };
            let _ = errors.send(Message::StreamError(failure));
        },
        None,
    )
}

/// `PcmCaptureProcessor`.
struct Processor {
    rate: u32,
    resampler: PcmResampler,
    meter_only: bool,
    events: futures::channel::mpsc::UnboundedSender<CaptureEvent>,
    chunk: Vec<f32>,
    resampled: Vec<f32>,
    recording: bool,
    flushed: bool,
    samples: usize,
    level_samples: u32,
    level_energy: f64,
    level: f64,
    native: Vec<f32>,
}

impl Processor {
    fn new(rate: u32, resampler: PcmResampler, meter_only: bool, events: futures::channel::mpsc::UnboundedSender<CaptureEvent>) -> Self {
        Self {
            rate,
            resampler,
            meter_only,
            events,
            chunk: Vec::with_capacity(CHUNK_SAMPLES),
            resampled: Vec::new(),
            recording: false,
            flushed: false,
            samples: 0,
            level_samples: 0,
            level_energy: 0.,
            level: 0.,
            native: Vec::new(),
        }
    }

    fn run(mut self, messages: mpsc::Receiver<Message>) {
        while let Ok(message) = messages.recv() {
            match message {
                Message::Audio(mono) => self.process(&mono),
                Message::Start => self.recording = true,
                Message::Stop => self.stop(),
                Message::StreamError(failure) => {
                    let _ = self.events.unbounded_send(CaptureEvent::Failed(failure));
                }
                Message::Close => return,
            }
        }
    }

    fn emit(&self, event: CaptureEvent) {
        let _ = self.events.unbounded_send(event);
    }

    fn stop(&mut self) {
        self.recording = false;
        if !self.meter_only && !self.flushed {
            let mut tail = Vec::new();
            self.resampler.flush(&mut tail);
            self.append(&tail);
        }
        self.flushed = true;
        if !self.chunk.is_empty() {
            self.publish();
        }
        if self.meter_only {
            let samples = std::mem::take(&mut self.native);
            self.emit(CaptureEvent::Recorded { samples, rate: self.rate });
        }
        self.emit(CaptureEvent::Stopped);
    }

    fn publish(&mut self) {
        let pcm = std::mem::replace(&mut self.chunk, Vec::with_capacity(CHUNK_SAMPLES));
        self.emit(CaptureEvent::Pcm(pcm));
    }

    fn append(&mut self, pcm: &[f32]) {
        for &sample in pcm {
            if self.samples == LIMIT_SAMPLES {
                self.recording = false;
                self.emit(CaptureEvent::Limit);
                break;
            }
            self.chunk.push(sample);
            self.samples += 1;
            if self.chunk.len() == CHUNK_SAMPLES {
                self.publish();
            }
        }
    }

    fn process(&mut self, mono: &[f32]) {
        if !self.recording {
            return;
        }
        // Metering must not wait for the half-second transcription chunks.
        let window = self.rate / 20;
        for &sample in mono {
            self.level_energy += f64::from(sample) * f64::from(sample);
            self.level_samples += 1;
            if self.level_samples >= window {
                let target = ((self.level_energy / f64::from(self.level_samples)).sqrt() * 4.).min(1.);
                let time_constant = if target > self.level { 0.075 } else { 0.2 };
                let decay = (-(f64::from(self.level_samples)) / f64::from(self.rate) / time_constant).exp();
                self.level = target + (self.level - target) * decay;
                self.emit(CaptureEvent::Level(self.level as f32));
                self.level_energy = 0.;
                self.level_samples = 0;
            }
        }
        if self.meter_only {
            self.native.extend_from_slice(mono);
        } else {
            let mut resampled = std::mem::take(&mut self.resampled);
            resampled.clear();
            self.resampler.push(mono, &mut resampled);
            self.append(&resampled);
            self.resampled = resampled;
        }
    }
}

/// Injected microphone audio for end-to-end checks: a stand-in device list and PCM fed at
/// the device boundary in real time, through the same mixing, metering and resampling.
#[cfg(feature = "test-support")]
pub mod test_source {
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::Duration;

    use super::Message;

    /// A stand-in microphone.
    #[derive(Clone, Debug)]
    pub struct TestSource {
        /// The device rate the samples are at.
        pub rate: u32,
        /// Mono samples, played once and then silence.
        pub samples: Arc<[f32]>,
        /// Device names the system reports.
        pub devices: Vec<String>,
        /// Refuse access, as a denied system permission does.
        pub deny: bool,
    }

    static SOURCE: Mutex<Option<TestSource>> = Mutex::new(None);

    /// Replaces the system microphone until cleared with `None`.
    pub fn set(source: Option<TestSource>) {
        *SOURCE.lock().expect("test source lock") = source;
    }

    pub fn current() -> Option<TestSource> {
        SOURCE.lock().expect("test source lock").clone()
    }

    /// Feeds 10 ms blocks on its own thread until dropped.
    pub struct Feeder(mpsc::Sender<()>);

    impl Feeder {
        pub(super) fn start(source: TestSource, feed: mpsc::Sender<Message>) -> Self {
            let (stop, stopped) = mpsc::channel();
            std::thread::spawn(move || {
                let block = (source.rate / 100) as usize;
                let mut offset = 0;
                loop {
                    if !matches!(stopped.recv_timeout(Duration::from_millis(10)), Err(mpsc::RecvTimeoutError::Timeout)) {
                        return;
                    }
                    let end = (offset + block).min(source.samples.len());
                    let mut data = source.samples[offset.min(end)..end].to_vec();
                    data.resize(block, 0.);
                    offset = end;
                    if feed.send(Message::Audio(data)).is_err() {
                        return;
                    }
                }
            });
            Self(stop)
        }
    }

    impl Drop for Feeder {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }
}

/// How long `stop` waits for `Stopped` (`Microphone capture did not stop.`).
pub const STOP_TIMEOUT: Duration = Duration::from_secs(5);
