use std::num::NonZero;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use librespot_playback::audio_backend::{Sink, SinkError, SinkResult};
use librespot_playback::convert::Converter;
use librespot_playback::decoder::AudioPacket;
use librespot_playback::{NUM_CHANNELS, SAMPLE_RATE};
use rodio::buffer::SamplesBuffer;
use rodio::{ChannelCount, SampleRate, Source};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::audio::{Output, Volume};
use crate::spectrum::Spectrum;

/// How many packets stay queued before a write blocks. This is what paces the decoder to real
/// time; deeper rides out network hiccups, but the decoder's position runs this far ahead of
/// what is heard.
const QUEUED_CHUNKS: usize = 26;
/// How long a blocked write sleeps between looks at the queue.
const DRAIN_POLL: Duration = Duration::from_millis(10);
/// How often a write asks the system whether the default device changed.
const DEVICE_POLL: Duration = Duration::from_millis(500);

/// Writes pass through and nothing is reported.
const OPEN: u8 = 0;
/// A clear was asked for and the engine has not announced the new track or position yet, so
/// every write is still the old one and is dropped.
const CLEARED: u8 = 1;
/// The engine announced the new track or position; the next write is its first audio.
const ARMED: u8 = 2;

/// The sink's link to the event stream. `clear` retires every packet queued so far and refuses
/// the ones the engine still writes from the old position, so the old audio stops when the user
/// acts. `arm` lets writes through again and has the first one reported; the stream arms it on
/// librespot's own `Playing` and `Seeked`, which come from the player thread strictly before the
/// write that follows, so the report can only mark audio from the new track or position. A
/// gapless segue and a resume from pause ask for no clear, so nothing of theirs is dropped.
#[derive(Clone)]
pub struct Cue {
    state: Arc<AtomicU8>,
    generation: Arc<AtomicU32>,
    written: UnboundedSender<()>,
}

impl Cue {
    pub fn new() -> (Self, UnboundedReceiver<()>) {
        let (written, receiver) = unbounded_channel();
        let cue = Self {
            state: Arc::default(),
            generation: Arc::default(),
            written,
        };
        (cue, receiver)
    }

    /// Moves the queue on a generation. Every chunk from before ends at its next sample, and
    /// nothing here touches rodio's own bookkeeping, which does not survive a clear that races
    /// a chunk ending on its own.
    pub fn clear(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.state.store(CLEARED, Ordering::Relaxed);
    }

    /// Lets writes through again and has the next one reported.
    pub fn arm(&self) {
        self.state.store(ARMED, Ordering::Relaxed);
    }

    /// Whether a clear is still waiting for the engine to announce where it went.
    pub fn cleared(&self) -> bool {
        self.state.load(Ordering::Relaxed) == CLEARED
    }

    /// Decides a write: refused after a clear, reported when armed, plain otherwise. A clear
    /// that lands during the armed write wins, since that packet predates the seek behind it.
    fn admit(&self) -> bool {
        match self.state.load(Ordering::Relaxed) {
            CLEARED => false,
            ARMED => {
                let armed = self
                    .state
                    .compare_exchange(ARMED, OPEN, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok();
                if armed {
                    self.written.send(()).ok();
                }
                armed
            }
            _ => true,
        }
    }
}

/// One packet in the output queue. It ends as soon as the cue moves on a generation and counts
/// itself out when dropped, whichever way it went, so `live` is the number still queued.
struct Chunk {
    samples: SamplesBuffer,
    born: u32,
    generation: Arc<AtomicU32>,
    live: Arc<AtomicUsize>,
}

impl Chunk {
    fn new(samples: SamplesBuffer, cue: &Cue, live: &Arc<AtomicUsize>) -> Self {
        live.fetch_add(1, Ordering::Relaxed);
        Self {
            samples,
            born: cue.generation.load(Ordering::Relaxed),
            generation: cue.generation.clone(),
            live: live.clone(),
        }
    }
}

impl Iterator for Chunk {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.generation.load(Ordering::Relaxed) != self.born {
            return None;
        }
        self.samples.next()
    }
}

impl Source for Chunk {
    fn current_span_len(&self) -> Option<usize> {
        self.samples.current_span_len()
    }

    fn channels(&self) -> ChannelCount {
        self.samples.channels()
    }

    fn sample_rate(&self) -> SampleRate {
        self.samples.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.samples.total_duration()
    }
}

impl Drop for Chunk {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::Relaxed);
    }
}

/// librespot's `Sink` over the shared rodio output. A write blocks once enough is queued, which
/// keeps the decoder in step with playback, and every write checks that the default device is
/// still the one the output was opened on.
pub struct OutputSink {
    output: Output,
    cue: Cue,
    /// Chunks appended and not yet played out or retired.
    live: Arc<AtomicUsize>,
    changed: UnboundedSender<()>,
    checked_at: Instant,
}

impl OutputSink {
    /// Claims the default output device, paused until librespot starts the sink.
    pub fn open(
        cue: Cue,
        volume: Volume,
        spectrum: Spectrum,
        changed: UnboundedSender<()>,
    ) -> Result<Self, SinkError> {
        let output = Output::open(volume, spectrum)
            .map_err(|error| SinkError::ConnectionRefused(error.to_string()))?;
        output.sink().pause();

        Ok(Self {
            output,
            cue,
            live: Arc::default(),
            changed,
            checked_at: Instant::now(),
        })
    }

    /// The sink librespot's player builder asks for. Without an output device it gets a silent
    /// one, so playback state still moves.
    pub fn boxed(
        cue: Cue,
        volume: Volume,
        spectrum: Spectrum,
        changed: UnboundedSender<()>,
    ) -> Box<dyn Sink> {
        match Self::open(cue.clone(), volume, spectrum, changed) {
            Ok(sink) => Box::new(sink),
            Err(error) => {
                log::error!("sink: cannot open an output device: {error}");
                Box::new(Silence(cue))
            }
        }
    }

    /// Whether the output failed or the default device is no longer the one it was opened on,
    /// asking the system at most every `DEVICE_POLL`.
    fn output_changed(&mut self) -> bool {
        let now = Instant::now();
        let changed = self.output.failed()
            || now.duration_since(self.checked_at) >= DEVICE_POLL && self.output.changed();
        if now.duration_since(self.checked_at) >= DEVICE_POLL {
            self.checked_at = now;
        }
        changed
    }

    /// Tells the engine the output is gone and hands librespot the error that pauses it.
    fn disconnected(&self) -> SinkError {
        self.changed.send(()).ok();
        SinkError::OnWrite("audio output changed".to_owned())
    }
}

impl Sink for OutputSink {
    fn start(&mut self) -> SinkResult<()> {
        if self.output.failed() || self.output.changed() {
            return Err(self.disconnected());
        }
        self.output.sink().play();
        Ok(())
    }

    fn stop(&mut self) -> SinkResult<()> {
        self.output.sink().pause();
        Ok(())
    }

    fn write(&mut self, packet: AudioPacket, converter: &mut Converter) -> SinkResult<()> {
        if self.output_changed() {
            return Err(self.disconnected());
        }

        if !self.cue.admit() {
            return Ok(());
        }

        let samples = packet
            .samples()
            .map_err(|error| SinkError::OnWrite(error.to_string()))?;
        let samples = converter.f64_to_f32(samples);
        let samples = SamplesBuffer::new(
            const { NonZero::new(NUM_CHANNELS as cpal::ChannelCount).unwrap() },
            const { NonZero::new(SAMPLE_RATE).unwrap() },
            &*samples,
        );
        self.output
            .sink()
            .append(Chunk::new(samples, &self.cue, &self.live));

        while self.live.load(Ordering::Relaxed) > QUEUED_CHUNKS {
            if self.output_changed() {
                return Err(self.disconnected());
            }
            std::thread::sleep(DRAIN_POLL);
        }
        Ok(())
    }
}

/// Swallows the audio when no output device opens. It still answers the cue, so playback state
/// moves on rather than waiting for sound that cannot come.
struct Silence(Cue);

impl Sink for Silence {
    fn write(&mut self, _packet: AudioPacket, _converter: &mut Converter) -> SinkResult<()> {
        self.0.admit();
        Ok(())
    }
}
