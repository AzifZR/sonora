//! A track downloaded while it plays. The decoder reads from the front of one buffer while the
//! response keeps filling the back, so playback starts after a short preroll instead of after
//! the whole file.

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::Duration;

use anyhow::{Result, bail};
use tokio::sync::watch;

/// How much has to be in before the decoder is let loose on the buffer. At any ordinary
/// bitrate this is several seconds of audio, and the download outruns playback many times
/// over, so the lead only grows from here.
const PREROLL: usize = 256 * 1024;

/// How long a read waits for bytes that have not arrived. rodio pulls the decoder on the
/// audio thread, so a wait there is silence: bounded, because a dead connection must not hold
/// the output for the rest of the session.
const PATIENCE: Duration = Duration::from_secs(20);

#[derive(Default)]
struct Buffered {
    bytes: Vec<u8>,
    total: Option<u64>,
    done: bool,
    failed: bool,
}

struct Shared {
    state: Mutex<Buffered>,
    filled: Condvar,
}

impl Shared {
    fn finish(&self, failed: bool) {
        let mut state = self.state.lock().unwrap();
        state.done = true;
        state.failed = failed;
        drop(state);
        self.filled.notify_all();
    }
}

/// One download in progress. Clones share the bytes, and `reader` hands out an independent
/// cursor over them, so a preloaded track can be decoded more than once.
#[derive(Clone)]
pub struct Stream {
    shared: Arc<Shared>,
}

impl Stream {
    /// Starts pulling `response` into a buffer and returns once the preroll is in, or the whole
    /// body for a track shorter than that. The download carries on in the background for as
    /// long as any reader or clone is alive; it stops on its own once all of them are gone.
    pub async fn open(response: reqwest::Response) -> Result<Self> {
        let total = response.content_length();
        let shared = Arc::new(Shared {
            state: Mutex::new(Buffered {
                total,
                ..Buffered::default()
            }),
            filled: Condvar::new(),
        });
        let (progress, mut watched) = watch::channel(0usize);
        tokio::spawn(pump(response, Arc::downgrade(&shared), progress));

        let stream = Self { shared };
        let wanted = total.map_or(PREROLL, |total| {
            usize::try_from(total).unwrap_or(usize::MAX)
        });
        let wanted = wanted.min(PREROLL);
        loop {
            let len = *watched.borrow_and_update();
            if len >= wanted || stream.done() {
                break;
            }
            if watched.changed().await.is_err() {
                break;
            }
        }
        if stream.failed() {
            bail!("the stream broke before playback could start");
        }
        Ok(stream)
    }

    pub fn reader(&self) -> Reader {
        Reader {
            shared: self.shared.clone(),
            at: 0,
        }
    }

    /// The body length the server announced, if it did.
    pub fn total(&self) -> Option<u64> {
        self.shared.state.lock().unwrap().total
    }

    fn done(&self) -> bool {
        self.shared.state.lock().unwrap().done
    }

    fn failed(&self) -> bool {
        self.shared.state.lock().unwrap().failed
    }
}

async fn pump(mut response: reqwest::Response, weak: Weak<Shared>, progress: watch::Sender<usize>) {
    loop {
        let chunk = match response.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) => {
                if let Some(shared) = weak.upgrade() {
                    shared.finish(false);
                }
                return;
            }
            Err(error) => {
                log::warn!("playback: the subsonic stream broke: {error}");
                if let Some(shared) = weak.upgrade() {
                    shared.finish(true);
                }
                return;
            }
        };
        let Some(shared) = weak.upgrade() else {
            return;
        };
        let len = {
            let mut state = shared.state.lock().unwrap();
            state.bytes.extend_from_slice(&chunk);
            state.bytes.len()
        };
        shared.filled.notify_all();
        progress.send(len).ok();
    }
}

/// A cursor over a `Stream`. Reading past what has arrived blocks the caller until more does,
/// so a decoder simply waits out a slow connection.
pub struct Reader {
    shared: Arc<Shared>,
    at: u64,
}

impl Reader {
    /// The end of the body: the announced length, or the final length once the download ends.
    /// A server that announced nothing makes this wait for the last byte.
    fn end(&self) -> u64 {
        let mut state = self.shared.state.lock().unwrap();
        loop {
            if let Some(total) = state.total {
                return total;
            }
            if state.done {
                return state.bytes.len() as u64;
            }
            let (held, timed_out) = self.shared.filled.wait_timeout(state, PATIENCE).unwrap();
            if timed_out.timed_out() {
                return state_len(&held);
            }
            state = held;
        }
    }
}

impl Read for Reader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut state = self.shared.state.lock().unwrap();
        loop {
            let len = state.bytes.len() as u64;
            if self.at < len {
                let start = self.at as usize;
                let count = buf.len().min(state.bytes.len() - start);
                buf[..count].copy_from_slice(&state.bytes[start..start + count]);
                self.at += count as u64;
                return Ok(count);
            }
            if state.failed {
                return Err(io::Error::other("the stream broke"));
            }
            if state.done {
                return Ok(0);
            }
            let (held, timed_out) = self.shared.filled.wait_timeout(state, PATIENCE).unwrap();
            if timed_out.timed_out() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the stream stopped arriving",
                ));
            }
            state = held;
        }
    }
}

fn state_len(state: &Buffered) -> u64 {
    state.bytes.len() as u64
}

impl Seek for Reader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::Current(delta) => i128::from(self.at) + i128::from(delta),
            SeekFrom::End(delta) => i128::from(self.end()) + i128::from(delta),
        };
        if target < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "cannot seek before the start",
            ));
        }
        self.at = target as u64;
        Ok(self.at)
    }
}
