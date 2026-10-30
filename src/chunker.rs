//! Splits one continuous 16 kHz stream into utterances for Whisper.
//!
//! While someone is talking, a `Partial` with everything heard so far is sent
//! every `partial_every` seconds, so the UI shows text almost immediately.
//! At a pause (or after `max_seconds`) a `Final` is sent and the buffer resets.
//! Audio with no frame above the speech threshold is dropped, because Whisper
//! makes up text for silence.

use crate::audio::dsp::{SAMPLE_RATE, rms};

const FRAME_SAMPLES: usize = (SAMPLE_RATE / 10) as usize; // 100 ms

#[derive(Debug, Clone)]
pub struct ChunkerConfig {
    /// A pause only ends an utterance once it is at least this long.
    pub min_seconds: f32,
    /// Utterances are cut here even without a pause.
    pub max_seconds: f32,
    /// Silence needed to end an utterance.
    pub pause_seconds: f32,
    /// How often to send the growing utterance for a live preview. 0 disables.
    pub partial_every: f32,
    /// RMS level that counts as speech.
    pub speech_threshold: f32,
}

impl Default for ChunkerConfig {
    fn default() -> Self {
        Self {
            min_seconds: 1.5,
            max_seconds: 20.0,
            pause_seconds: 0.6,
            partial_every: 1.0,
            speech_threshold: 0.008,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkKind {
    Partial,
    Final,
}

#[derive(Debug, Clone)]
pub struct Chunk {
    /// Increments per utterance; partials and the final of one utterance share it.
    pub utterance: u64,
    pub kind: ChunkKind,
    /// Seconds from the start of the stream.
    pub start: f32,
    pub samples: Vec<f32>,
}

impl Chunk {
    pub fn end(&self) -> f32 {
        self.start + self.samples.len() as f32 / SAMPLE_RATE as f32
    }
}

pub struct Chunker {
    cfg: ChunkerConfig,
    pending: Vec<f32>,
    buffer: Vec<f32>,
    /// Sample index (in the whole stream) where `buffer` starts.
    buffer_start: usize,
    consumed: usize,
    silent_frames: usize,
    has_speech: bool,
    last_partial_len: usize,
    utterance: u64,
}

impl Chunker {
    pub fn new(cfg: ChunkerConfig) -> Self {
        Self::with_first_utterance(cfg, 0)
    }

    pub fn with_first_utterance(cfg: ChunkerConfig, first: u64) -> Self {
        Self {
            cfg,
            pending: Vec::new(),
            buffer: Vec::new(),
            buffer_start: 0,
            consumed: 0,
            silent_frames: 0,
            has_speech: false,
            last_partial_len: 0,
            utterance: first,
        }
    }

    /// Feeds 16 kHz mono samples; returns the chunks that became ready.
    pub fn push(&mut self, samples: &[f32]) -> Vec<Chunk> {
        self.pending.extend_from_slice(samples);
        let mut out = Vec::new();
        let mut offset = 0;
        while self.pending.len() - offset >= FRAME_SAMPLES {
            let frame: Vec<f32> = self.pending[offset..offset + FRAME_SAMPLES].to_vec();
            self.add_frame(&frame, &mut out);
            offset += FRAME_SAMPLES;
        }
        self.pending.drain(..offset);
        out
    }

    /// Ends the current utterance, e.g. when recording stops.
    pub fn flush(&mut self) -> Vec<Chunk> {
        let rest = std::mem::take(&mut self.pending);
        if !rest.is_empty() {
            if self.buffer.is_empty() {
                self.buffer_start = self.consumed;
            }
            if rms(&rest) >= self.cfg.speech_threshold {
                self.has_speech = true;
            }
            self.consumed += rest.len();
            self.buffer.extend_from_slice(&rest);
        }
        let mut out = Vec::new();
        self.finish(&mut out);
        out
    }

    fn pause_frames(&self) -> usize {
        (self.cfg.pause_seconds * 10.0).ceil() as usize
    }

    fn add_frame(&mut self, frame: &[f32], out: &mut Vec<Chunk>) {
        if self.buffer.is_empty() {
            self.buffer_start = self.consumed;
        }
        self.consumed += frame.len();
        self.buffer.extend_from_slice(frame);

        if rms(frame) >= self.cfg.speech_threshold {
            self.has_speech = true;
            self.silent_frames = 0;
        } else {
            self.silent_frames += 1;
        }

        // Silence before anyone speaks is not kept.
        if !self.has_speech {
            if self.silent_frames >= self.pause_frames() {
                self.reset();
            }
            return;
        }

        let rate = SAMPLE_RATE as f32;
        let len = self.buffer.len();
        let paused = self.silent_frames >= self.pause_frames() && len as f32 >= self.cfg.min_seconds * rate;
        if paused || len as f32 >= self.cfg.max_seconds * rate {
            self.finish(out);
            return;
        }

        let partial_samples = (self.cfg.partial_every * rate) as usize;
        if partial_samples > 0 && self.silent_frames == 0 && len - self.last_partial_len >= partial_samples {
            self.last_partial_len = len;
            out.push(self.chunk(ChunkKind::Partial));
        }
    }

    fn chunk(&self, kind: ChunkKind) -> Chunk {
        Chunk {
            utterance: self.utterance,
            kind,
            start: self.buffer_start as f32 / SAMPLE_RATE as f32,
            samples: self.buffer.clone(),
        }
    }

    fn finish(&mut self, out: &mut Vec<Chunk>) {
        if self.has_speech && !self.buffer.is_empty() {
            out.push(self.chunk(ChunkKind::Final));
            self.utterance += 1;
        }
        self.reset();
    }

    fn reset(&mut self) {
        self.buffer.clear();
        self.silent_frames = 0;
        self.has_speech = false;
        self.last_partial_len = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f32 = SAMPLE_RATE as f32;

    fn tone(seconds: f32) -> Vec<f32> {
        (0..(seconds * RATE) as usize)
            .map(|i| 0.2 * (2.0 * std::f32::consts::PI * 220.0 * i as f32 / RATE).sin())
            .collect()
    }

    fn silence(seconds: f32) -> Vec<f32> {
        vec![0.0; (seconds * RATE) as usize]
    }

    fn no_partials() -> ChunkerConfig {
        ChunkerConfig {
            partial_every: 0.0,
            ..Default::default()
        }
    }

    fn finals(chunks: &[Chunk]) -> Vec<&Chunk> {
        chunks.iter().filter(|c| c.kind == ChunkKind::Final).collect()
    }

    #[test]
    fn one_final_per_utterance_cut_at_the_pause() {
        let mut c = Chunker::new(no_partials());
        let mut out = c.push(&tone(2.0));
        out.extend(c.push(&silence(1.0)));
        out.extend(c.push(&tone(2.0)));
        out.extend(c.push(&silence(1.0)));
        assert_eq!(out.len(), 2);
        assert!((out[0].end() - 2.6).abs() < 0.05, "{}", out[0].end());
        assert!((out[1].start - 2.6).abs() < 0.05, "{}", out[1].start);
        assert_eq!((out[0].utterance, out[1].utterance), (0, 1));
    }

    #[test]
    fn silence_is_dropped() {
        let mut c = Chunker::new(no_partials());
        let mut out = c.push(&silence(30.0));
        out.extend(c.flush());
        assert!(out.is_empty());
    }

    #[test]
    fn leading_silence_is_trimmed_but_time_stays_absolute() {
        let mut c = Chunker::new(no_partials());
        c.push(&silence(5.0));
        let mut out = c.push(&tone(2.0));
        out.extend(c.push(&silence(1.0)));
        assert_eq!(out.len(), 1);
        assert!((4.4..=5.0).contains(&out[0].start), "{}", out[0].start);
    }

    #[test]
    fn long_speech_is_cut_at_max_seconds() {
        let mut c = Chunker::new(ChunkerConfig {
            max_seconds: 5.0,
            ..no_partials()
        });
        let out = c.push(&tone(12.0));
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].samples.len(), 5 * SAMPLE_RATE as usize);
    }

    #[test]
    fn partials_grow_then_final_closes_the_utterance() {
        let mut c = Chunker::new(ChunkerConfig::default());
        let mut out = c.push(&tone(3.5));
        out.extend(c.push(&silence(1.0)));
        let partials: Vec<_> = out.iter().filter(|c| c.kind == ChunkKind::Partial).collect();
        assert_eq!(partials.len(), 3);
        assert!(partials.windows(2).all(|w| w[1].samples.len() > w[0].samples.len()));
        assert!(out.iter().all(|c| c.utterance == 0));
        assert_eq!(out.last().unwrap().kind, ChunkKind::Final);
        assert_eq!(finals(&out).len(), 1);
    }

    #[test]
    fn short_utterance_waits_and_flush_emits_it() {
        let mut c = Chunker::new(no_partials());
        let mut out = c.push(&tone(0.5));
        out.extend(c.push(&silence(0.7)));
        assert!(out.is_empty());
        out.extend(c.flush());
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn odd_sized_input_blocks() {
        let mut c = Chunker::new(no_partials());
        let mut audio = tone(2.0);
        audio.extend(silence(1.0));
        let mut out = Vec::new();
        for block in audio.chunks(1365) {
            out.extend(c.push(block));
        }
        assert_eq!(out.len(), 1);
    }
}
