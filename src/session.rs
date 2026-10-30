//! A recording session: captures -> chunkers -> transcriber jobs.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::JoinHandle;
use std::time::Instant;

use anyhow::{Result, bail};
use crossbeam_channel::{Receiver, Sender};

use crate::audio::dsp::{SAMPLE_RATE, rms};
use crate::audio::{AudioPacket, Capture, fake_source, start_file, start_mic, start_system_audio};
use crate::chunker::{Chunker, ChunkerConfig};
use crate::transcriber::Job;
use crate::transcript::Speaker;

/// Live input levels for the meters, stored as f32 bits.
#[derive(Default)]
pub struct Levels([AtomicU32; 2]);

impl Levels {
    pub fn get(&self, speaker: Speaker) -> f32 {
        f32::from_bits(self.0[speaker.index()].load(Ordering::Relaxed))
    }

    fn set(&self, speaker: Speaker, value: f32) {
        self.0[speaker.index()].store(value.to_bits(), Ordering::Relaxed);
    }
}

pub struct SessionConfig {
    pub capture_me: bool,
    pub capture_others: bool,
    pub chunker: ChunkerConfig,
    /// Added to every timestamp, so a resumed session continues the clock.
    pub time_offset: f32,
    /// First utterance id; keeps ids unique across sessions.
    pub first_utterance: u64,
    /// None for automatic language detection, or a code such as "en".
    pub language: Option<String>,
}

pub struct Session {
    captures: Vec<Capture>,
    stop: Option<Sender<()>>,
    router: Option<JoinHandle<()>>,
    pub levels: Arc<Levels>,
    pub started: Instant,
    /// Sources that failed to start, with the reason.
    pub warnings: Vec<String>,
    pub source_names: Vec<String>,
}

impl Session {
    pub fn start(cfg: SessionConfig, jobs: Sender<Job>, notify: impl Fn() + Send + 'static) -> Result<Self> {
        if !cfg.capture_me && !cfg.capture_others {
            bail!("turn on at least one source: Me or Others");
        }
        let (audio_tx, audio_rx) = crossbeam_channel::unbounded();
        let mut captures = Vec::new();
        let mut warnings = Vec::new();
        if cfg.capture_me {
            let result = match fake_source(Speaker::Me) {
                Some(path) => start_file(path, Speaker::Me, audio_tx.clone()),
                None => start_mic(audio_tx.clone()),
            };
            match result {
                Ok(c) => captures.push(c),
                Err(e) => warnings.push(format!("Microphone: {e:#}")),
            }
        }
        if cfg.capture_others {
            let result = match fake_source(Speaker::Others) {
                Some(path) => start_file(path, Speaker::Others, audio_tx.clone()),
                None => start_system_audio(audio_tx.clone()),
            };
            match result {
                Ok(c) => captures.push(c),
                Err(e) => warnings.push(format!("System audio: {e:#}")),
            }
        }
        drop(audio_tx);
        if captures.is_empty() {
            bail!("{}", warnings.join("\n"));
        }

        let levels = Arc::new(Levels::default());
        let started = Instant::now();
        let (stop_tx, stop_rx) = crossbeam_channel::bounded(0);
        let router = spawn_router(audio_rx, stop_rx, jobs, cfg, started, levels.clone(), notify);
        Ok(Self {
            source_names: captures.iter().map(|c| c.name.clone()).collect(),
            captures,
            stop: Some(stop_tx),
            router: Some(router),
            levels,
            started,
            warnings,
        })
    }

    /// Stops capturing. Speech still buffered is sent for transcription.
    pub fn stop(mut self) {
        self.captures.clear();
        drop(self.stop.take());
        if let Some(r) = self.router.take() {
            let _ = r.join();
        }
    }
}

/// Routes audio packets to one chunker per speaker and chunks to the transcriber.
/// Runs until every capture has been dropped, then flushes.
fn spawn_router(
    audio: Receiver<AudioPacket>,
    stop: Receiver<()>,
    jobs: Sender<Job>,
    cfg: SessionConfig,
    started: Instant,
    levels: Arc<Levels>,
    notify: impl Fn() + Send + 'static,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("audio-router".into())
        .spawn(move || {
            let mut router = Router::new(&cfg);
            let handle = |packet: AudioPacket, router: &mut Router| {
                levels.set(packet.speaker, rms(&packet.samples));
                let elapsed = started.elapsed().as_secs_f32();
                for job in router.push(packet, elapsed) {
                    let _ = jobs.send(job);
                }
            };
            loop {
                crossbeam_channel::select! {
                    recv(audio) -> packet => match packet {
                        Ok(packet) => handle(packet, &mut router),
                        Err(_) => break,
                    },
                    // Fires when the session drops its sender.
                    recv(stop) -> _ => break,
                }
                notify();
            }
            while let Ok(packet) = audio.try_recv() {
                handle(packet, &mut router);
            }
            for job in router.flush() {
                let _ = jobs.send(job);
            }
            levels.set(Speaker::Me, 0.0);
            levels.set(Speaker::Others, 0.0);
            notify();
        })
        .expect("failed to start audio router thread")
}

struct Source {
    chunker: Chunker,
    /// Session time at which this source's first sample was captured.
    offset: Option<f32>,
}

/// Pure routing logic, separated from threads for testing.
pub struct Router {
    sources: [Source; 2],
    time_offset: f32,
    language: Option<String>,
}

impl Router {
    pub fn new(cfg: &SessionConfig) -> Self {
        let source = |speaker: Speaker| Source {
            // Separate id ranges per speaker keep utterance ids unique.
            chunker: Chunker::with_first_utterance(
                cfg.chunker.clone(),
                cfg.first_utterance + speaker.index() as u64 * 1_000_000,
            ),
            offset: None,
        };
        Self {
            sources: [source(Speaker::Me), source(Speaker::Others)],
            time_offset: cfg.time_offset,
            language: cfg.language.clone(),
        }
    }

    /// `elapsed`: session seconds when the packet arrived.
    pub fn push(&mut self, packet: AudioPacket, elapsed: f32) -> Vec<Job> {
        let speaker = packet.speaker;
        let language = &self.language;
        let source = &mut self.sources[speaker.index()];
        let packet_seconds = packet.samples.len() as f32 / SAMPLE_RATE as f32;
        let offset = *source
            .offset
            .get_or_insert((elapsed - packet_seconds).max(0.0) + self.time_offset);
        source
            .chunker
            .push(&packet.samples)
            .into_iter()
            .map(|mut chunk| {
                chunk.start += offset;
                Job {
                    speaker,
                    chunk,
                    language: language.clone(),
                }
            })
            .collect()
    }

    pub fn flush(&mut self) -> Vec<Job> {
        let mut out = Vec::new();
        for (speaker, source) in [Speaker::Me, Speaker::Others].into_iter().zip(&mut self.sources) {
            let offset = source.offset.unwrap_or(self.time_offset);
            out.extend(source.chunker.flush().into_iter().map(|mut chunk| {
                chunk.start += offset;
                Job {
                    speaker,
                    chunk,
                    language: self.language.clone(),
                }
            }));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunker::ChunkKind;

    fn cfg() -> SessionConfig {
        SessionConfig {
            capture_me: true,
            capture_others: true,
            chunker: ChunkerConfig {
                partial_every: 0.0,
                ..Default::default()
            },
            time_offset: 100.0,
            first_utterance: 5,
            language: Some("en".into()),
        }
    }

    fn packet(speaker: Speaker, seconds: f32, loud: bool) -> AudioPacket {
        let n = (seconds * SAMPLE_RATE as f32) as usize;
        let samples = (0..n)
            .map(|i| if loud { 0.2 * (i as f32 * 0.09).sin() } else { 0.0 })
            .collect();
        AudioPacket { speaker, samples }
    }

    #[test]
    fn speakers_are_chunked_separately_with_session_time() {
        let mut r = Router::new(&cfg());
        let mut jobs = Vec::new();
        // "Others" starts 1 s into the session; "Me" at the very start.
        jobs.extend(r.push(packet(Speaker::Me, 2.0, true), 2.0));
        jobs.extend(r.push(packet(Speaker::Others, 2.0, true), 3.0));
        jobs.extend(r.push(packet(Speaker::Me, 1.0, false), 3.0));
        jobs.extend(r.push(packet(Speaker::Others, 1.0, false), 4.0));
        assert_eq!(jobs.len(), 2);
        let me = jobs.iter().find(|j| j.speaker == Speaker::Me).unwrap();
        let others = jobs.iter().find(|j| j.speaker == Speaker::Others).unwrap();
        assert!((me.chunk.start - 100.0).abs() < 0.01);
        assert!((others.chunk.start - 101.0).abs() < 0.01);
        assert_ne!(me.chunk.utterance, others.chunk.utterance);
        assert!(me.chunk.utterance >= 5);
        assert_eq!(me.language.as_deref(), Some("en"));
    }

    #[test]
    fn flush_sends_unfinished_speech() {
        let mut r = Router::new(&cfg());
        assert!(r.push(packet(Speaker::Others, 0.5, true), 0.5).is_empty());
        let jobs = r.flush();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].speaker, Speaker::Others);
        assert_eq!(jobs[0].chunk.kind, ChunkKind::Final);
    }
}
