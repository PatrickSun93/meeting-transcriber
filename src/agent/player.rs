//! Plays the agent's voice: sentences queue up and play one after the
//! other; `stop` empties the queue at once (barge-in) and says how much of
//! the sentence that was playing had been heard.
//!
//! `Output` is what the conversation loop needs from a voice's way out, so
//! another one (a phone line) can stand in for the speakers.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context as _, Result, anyhow, bail};
use crossbeam_channel::Sender;

/// One sentence of speech.
#[derive(Debug, Clone)]
pub struct Segment {
    pub turn: u64,
    pub index: usize,
    /// Mono at the output's rate.
    pub samples: Vec<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlayEvent {
    /// The first sample of a segment went out.
    Started { turn: u64, index: usize, at: Instant },
    /// Its last sample went out.
    Finished { turn: u64, index: usize },
}

/// Where `stop` cut the voice off.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cut {
    pub turn: u64,
    pub index: usize,
    /// How much of the segment had been played, 0 to 1.
    pub played: f32,
}

pub trait Output: Send + Sync {
    /// Samples per second the segments must have.
    fn rate(&self) -> u32;
    fn play(&self, segment: Segment);
    /// Silences everything queued; the segment that was playing, if any.
    fn stop(&self) -> Option<Cut>;
}

/// The queue, shared with whatever pulls samples from it.
pub struct Queue {
    segments: VecDeque<Segment>,
    pos: usize,
    started: bool,
    events: Sender<PlayEvent>,
}

impl Queue {
    pub fn new(events: Sender<PlayEvent>) -> Self {
        Self {
            segments: VecDeque::new(),
            pos: 0,
            started: false,
            events,
        }
    }

    pub fn push(&mut self, segment: Segment) {
        self.segments.push_back(segment);
    }

    /// The next sample to play: silence when nothing is queued.
    pub fn next_sample(&mut self) -> f32 {
        loop {
            let Some(seg) = self.segments.front() else {
                return 0.0;
            };
            let (turn, index) = (seg.turn, seg.index);
            if !self.started {
                self.started = true;
                let _ = self.events.send(PlayEvent::Started {
                    turn,
                    index,
                    at: Instant::now(),
                });
            }
            if let Some(&s) = seg.samples.get(self.pos) {
                self.pos += 1;
                return s;
            }
            let _ = self.events.send(PlayEvent::Finished { turn, index });
            self.segments.pop_front();
            self.pos = 0;
            self.started = false;
        }
    }

    pub fn stop(&mut self) -> Option<Cut> {
        let cut = self.segments.front().filter(|_| self.started).map(|s| Cut {
            turn: s.turn,
            index: s.index,
            played: self.pos as f32 / s.samples.len().max(1) as f32,
        });
        self.segments.clear();
        self.pos = 0;
        self.started = false;
        cut
    }
}

/// The default output device, through cpal.
pub struct Speakers {
    queue: Arc<Mutex<Queue>>,
    rate: u32,
    pub name: String,
    stop: Option<Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

fn lock(q: &Mutex<Queue>) -> std::sync::MutexGuard<'_, Queue> {
    q.lock().unwrap_or_else(|e| e.into_inner())
}

impl Speakers {
    pub fn open(events: Sender<PlayEvent>) -> Result<Self> {
        let queue = Arc::new(Mutex::new(Queue::new(events)));
        // As with the microphone: the stream stays on the thread that made it.
        let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<(u32, String)>>(1);
        let (stop_tx, stop_rx) = crossbeam_channel::bounded::<()>(0);
        let q = queue.clone();
        let thread = std::thread::Builder::new()
            .name("agent voice".into())
            .spawn(move || match open_stream(q) {
                Ok((stream, rate, name)) => {
                    let _ = ready_tx.send(Ok((rate, name)));
                    let _ = stop_rx.recv();
                    drop(stream);
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                }
            })
            .context("could not start the voice thread")?;
        let (rate, name) = ready_rx
            .recv()
            .map_err(|_| anyhow!("the voice thread ended unexpectedly"))??;
        Ok(Self {
            queue,
            rate,
            name,
            stop: Some(stop_tx),
            thread: Some(thread),
        })
    }
}

impl Drop for Speakers {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Output for Speakers {
    fn rate(&self) -> u32 {
        self.rate
    }

    fn play(&self, segment: Segment) {
        lock(&self.queue).push(segment);
    }

    fn stop(&self) -> Option<Cut> {
        lock(&self.queue).stop()
    }
}

fn open_stream(queue: Arc<Mutex<Queue>>) -> Result<(cpal::Stream, u32, String)> {
    use cpal::SampleFormat;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .context("no speakers or headphones found")?;
    let config = device
        .default_output_config()
        .context("could not read the speakers' audio format")?;
    let name = device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "speakers".into());
    let rate = config.sample_rate();
    let channels = config.channels() as usize;
    let cfg = config.config();
    let stream = match config.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, cfg, channels, queue)?,
        SampleFormat::I16 => build::<i16>(&device, cfg, channels, queue)?,
        SampleFormat::I32 => build::<i32>(&device, cfg, channels, queue)?,
        SampleFormat::U16 => build::<u16>(&device, cfg, channels, queue)?,
        other => bail!("unsupported speaker sample format {other}"),
    };
    stream.play().context("could not start the speakers")?;
    Ok((stream, rate, name))
}

fn build<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    channels: usize,
    queue: Arc<Mutex<Queue>>,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    use cpal::traits::DeviceTrait;
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                let mut q = lock(&queue);
                for frame in data.chunks_mut(channels.max(1)) {
                    let s = T::from_sample(q.next_sample());
                    frame.fill(s);
                }
            },
            |err| eprintln!("speaker error: {err}"),
            None,
        )
        .context("could not open the speakers")
}

/// No sound: takes samples off the queue in real time, as speakers
/// would, so the timing and the events are the same. For tests, and for
/// running where nobody listens.
pub struct Clock {
    queue: Arc<Mutex<Queue>>,
    rate: u32,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl Clock {
    pub fn start(rate: u32, events: Sender<PlayEvent>) -> Self {
        let queue = Arc::new(Mutex::new(Queue::new(events)));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (q, done) = (queue.clone(), stop.clone());
        std::thread::spawn(move || {
            let step = std::time::Duration::from_millis(10);
            let per_step = (rate / 100) as usize;
            let started = Instant::now();
            let mut n = 0u32;
            while !done.load(std::sync::atomic::Ordering::Relaxed) {
                n += 1;
                let due = started + step * n;
                std::thread::sleep(due.saturating_duration_since(Instant::now()));
                let mut q = lock(&q);
                for _ in 0..per_step {
                    q.next_sample();
                }
            }
        });
        Self { queue, rate, stop }
    }
}

impl Drop for Clock {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Output for Clock {
    fn rate(&self) -> u32 {
        self.rate
    }

    fn play(&self, segment: Segment) {
        lock(&self.queue).push(segment);
    }

    fn stop(&self) -> Option<Cut> {
        lock(&self.queue).stop()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(turn: u64, index: usize, len: usize) -> Segment {
        Segment {
            turn,
            index,
            samples: vec![0.5; len],
        }
    }

    #[test]
    fn segments_play_in_order_and_report_start_and_end() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut q = Queue::new(tx);
        assert_eq!(q.next_sample(), 0.0, "silence while empty");
        q.push(seg(1, 0, 2));
        q.push(seg(1, 1, 1));
        let played: Vec<f32> = (0..4).map(|_| q.next_sample()).collect();
        assert_eq!(played, [0.5, 0.5, 0.5, 0.0]);
        let events: Vec<(bool, usize)> = rx
            .try_iter()
            .map(|e| match e {
                PlayEvent::Started { index, .. } => (true, index),
                PlayEvent::Finished { index, .. } => (false, index),
            })
            .collect();
        assert_eq!(events, [(true, 0), (false, 0), (true, 1), (false, 1)]);
    }

    #[test]
    fn stop_empties_the_queue_and_says_how_much_was_heard() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut q = Queue::new(tx);
        q.push(seg(3, 0, 4));
        q.push(seg(3, 1, 4));
        assert_eq!(q.stop(), None, "nothing had started");
        q.push(seg(3, 0, 4));
        q.push(seg(3, 1, 4));
        q.next_sample();
        assert_eq!(
            q.stop(),
            Some(Cut {
                turn: 3,
                index: 0,
                played: 0.25
            })
        );
        assert_eq!(q.next_sample(), 0.0);
        assert!(
            !rx.try_iter().any(|e| matches!(e, PlayEvent::Finished { .. })),
            "a cut segment never finished"
        );
    }
}
