//! Runs speech-to-text on a background thread.
//!
//! Final chunks are always transcribed, in order. Partial chunks only drive
//! the live preview, so when the engine falls behind, older partials are
//! skipped and only the newest one per speaker is kept.

use std::collections::VecDeque;
use std::path::Path;
use std::thread::JoinHandle;
use std::time::Instant;

use anyhow::{Context as _, Result};
use crossbeam_channel::{Receiver, Sender};

use crate::audio::dsp::SAMPLE_RATE;
use crate::chunker::{Chunk, ChunkKind};
use crate::transcript::{Line, Speaker, clean_text};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Recognized {
    pub text: String,
    pub language: Option<String>,
}

pub trait SpeechToText: Send {
    /// `language`: None to detect it, or a code such as "en" or "zh".
    fn transcribe(&mut self, samples: &[f32], language: Option<&str>) -> Result<Recognized>;
}

/// The GPU backend whisper.cpp was built with, if any: Metal on macOS,
/// Vulkan or CUDA when the matching feature is on.
pub const GPU: Option<&str> = if cfg!(target_os = "macos") {
    Some("Metal")
} else if cfg!(feature = "cuda") {
    Some("CUDA")
} else if cfg!(feature = "vulkan") {
    Some("Vulkan")
} else {
    None
};

/// Whisper via whisper.cpp, on the GPU when it was built with one (see `GPU`).
pub struct WhisperEngine {
    state: whisper_rs::WhisperState,
    threads: i32,
}

/// Segments Whisper itself rates as probably silent are dropped.
const NO_SPEECH_LIMIT: f32 = 0.6;

impl WhisperEngine {
    pub fn load(model: &Path) -> Result<Self> {
        whisper_rs::install_logging_hooks();
        let mut params = whisper_rs::WhisperContextParameters::default();
        params.flash_attn(true);
        let path = model.to_str().context("model path is not valid UTF-8")?;
        let ctx = whisper_rs::WhisperContext::new_with_params(path, params)
            .with_context(|| format!("could not load Whisper model {}", model.display()))?;
        let state = ctx.create_state().context("could not create Whisper state")?;
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8)) as i32;
        Ok(Self { state, threads })
    }
}

impl SpeechToText for WhisperEngine {
    fn transcribe(&mut self, samples: &[f32], language: Option<&str>) -> Result<Recognized> {
        use whisper_rs::{FullParams, SamplingStrategy};

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(language.unwrap_or("auto")));
        params.set_n_threads(self.threads);
        params.set_translate(false);
        params.set_no_context(true);
        params.set_no_timestamps(true);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        // whisper.cpp ignores input shorter than one second; pad with silence.
        let min_len = SAMPLE_RATE as usize + SAMPLE_RATE as usize / 10;
        let padded;
        let input = if samples.len() < min_len {
            let mut v = samples.to_vec();
            v.resize(min_len, 0.0);
            padded = v;
            &padded[..]
        } else {
            samples
        };

        self.state.full(params, input).context("Whisper failed")?;

        let mut text = String::new();
        for segment in self.state.as_iter() {
            if segment.no_speech_probability() > NO_SPEECH_LIMIT {
                continue;
            }
            text.push_str(&segment.to_str_lossy().unwrap_or_default());
        }
        let language = whisper_rs::get_lang_str(self.state.full_lang_id_from_state()).map(str::to_string);
        Ok(Recognized { text, language })
    }
}

#[derive(Debug, Clone)]
pub struct Job {
    pub speaker: Speaker,
    pub chunk: Chunk,
    pub language: Option<String>,
}

/// Pending work: finals in arrival order, plus the newest partial per speaker.
#[derive(Default)]
pub struct JobQueue {
    finals: VecDeque<Job>,
    partials: [Option<Job>; 2],
}

impl JobQueue {
    pub fn push(&mut self, job: Job) {
        let slot = job.speaker.index();
        match job.chunk.kind {
            ChunkKind::Final => {
                // The final covers the same audio as any waiting partial.
                if self.partials[slot]
                    .as_ref()
                    .is_some_and(|p| p.chunk.utterance <= job.chunk.utterance)
                {
                    self.partials[slot] = None;
                }
                self.finals.push_back(job);
            }
            ChunkKind::Partial => self.partials[slot] = Some(job),
        }
    }

    pub fn pop(&mut self) -> Option<Job> {
        if let Some(job) = self.finals.pop_front() {
            return Some(job);
        }
        // Serve whichever waiting partial started earlier.
        let pick = match (&self.partials[0], &self.partials[1]) {
            (Some(a), Some(b)) => usize::from(b.chunk.start < a.chunk.start),
            (Some(_), None) => 0,
            (None, Some(_)) => 1,
            (None, None) => return None,
        };
        self.partials[pick].take()
    }

    pub fn is_empty(&self) -> bool {
        self.finals.is_empty() && self.partials.iter().all(Option::is_none)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    /// The model has loaded and jobs can be processed.
    Ready,
    /// The model could not be loaded; this worker has stopped.
    LoadFailed(String),
    Preview {
        speaker: Speaker,
        utterance: u64,
        start: f32,
        text: Option<String>,
    },
    Final {
        speaker: Speaker,
        utterance: u64,
        line: Option<Line>,
    },
    /// How long the last final took relative to its audio length.
    Speed {
        audio_seconds: f32,
        seconds: f32,
    },
    Error(String),
}

/// Starts the worker: it loads the engine, sends `Ready` (or `LoadFailed`),
/// then works through jobs. It stops when `jobs` is disconnected and all
/// finals are done.
pub fn spawn_worker(
    load: impl FnOnce() -> Result<Box<dyn SpeechToText>> + Send + 'static,
    jobs: Receiver<Job>,
    outputs: Sender<Output>,
    notify: impl Fn() + Send + 'static,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("transcriber".into())
        .spawn(move || {
            let mut engine = match load() {
                Ok(engine) => {
                    let _ = outputs.send(Output::Ready);
                    notify();
                    engine
                }
                Err(e) => {
                    let _ = outputs.send(Output::LoadFailed(format!("{e:#}")));
                    notify();
                    return;
                }
            };
            let mut queue = JobQueue::default();
            loop {
                if queue.is_empty() {
                    match jobs.recv() {
                        Ok(job) => queue.push(job),
                        Err(_) => break,
                    }
                }
                while let Ok(job) = jobs.try_recv() {
                    queue.push(job);
                }
                let Some(job) = queue.pop() else { continue };
                let output = run_job(engine.as_mut(), job, &outputs);
                if outputs.send(output).is_err() {
                    break;
                }
                notify();
            }
        })
        .expect("failed to start transcriber thread")
}

/// Transcribes one job. Timing for finals is sent on `outputs` as `Speed`.
pub fn run_job(engine: &mut dyn SpeechToText, job: Job, outputs: &Sender<Output>) -> Output {
    let Job {
        speaker,
        chunk,
        language,
    } = job;
    let started = Instant::now();
    let result = engine.transcribe(&chunk.samples, language.as_deref());
    match (chunk.kind, result) {
        (_, Err(e)) => Output::Error(format!("{e:#}")),
        (ChunkKind::Partial, Ok(r)) => Output::Preview {
            speaker,
            utterance: chunk.utterance,
            start: chunk.start,
            text: clean_text(&r.text),
        },
        (ChunkKind::Final, Ok(r)) => {
            let _ = outputs.send(Output::Speed {
                audio_seconds: chunk.samples.len() as f32 / SAMPLE_RATE as f32,
                seconds: started.elapsed().as_secs_f32(),
            });
            Output::Final {
                speaker,
                utterance: chunk.utterance,
                line: clean_text(&r.text).map(|text| Line {
                    speaker,
                    start: chunk.start,
                    end: chunk.end(),
                    text,
                    language: r.language,
                }),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(speaker: Speaker, utterance: u64, kind: ChunkKind, start: f32) -> Job {
        Job {
            speaker,
            chunk: Chunk {
                utterance,
                kind,
                start,
                samples: vec![0.0; 16_000],
            },
            language: None,
        }
    }

    fn describe(job: Option<Job>) -> Option<(Speaker, u64, ChunkKind)> {
        job.map(|j| (j.speaker, j.chunk.utterance, j.chunk.kind))
    }

    #[test]
    fn finals_first_in_order_and_only_newest_partial_kept() {
        use ChunkKind::*;
        use Speaker::*;
        let mut q = JobQueue::default();
        q.push(job(Me, 0, Partial, 0.0));
        q.push(job(Me, 0, Partial, 0.0));
        q.push(job(Others, 4, Partial, 1.0));
        q.push(job(Others, 3, Final, 0.5));
        q.push(job(Me, 1, Partial, 2.0));
        q.push(job(Me, 0, Final, 0.0));
        assert_eq!(describe(q.pop()), Some((Others, 3, Final)));
        assert_eq!(describe(q.pop()), Some((Me, 0, Final)));
        assert_eq!(describe(q.pop()), Some((Others, 4, Partial)));
        assert_eq!(describe(q.pop()), Some((Me, 1, Partial)));
        assert!(q.pop().is_none());
        assert!(q.is_empty());
    }

    #[test]
    fn a_final_discards_the_partial_it_supersedes() {
        let mut q = JobQueue::default();
        q.push(job(Speaker::Me, 2, ChunkKind::Partial, 0.0));
        q.push(job(Speaker::Me, 2, ChunkKind::Final, 0.0));
        assert_eq!(describe(q.pop()), Some((Speaker::Me, 2, ChunkKind::Final)));
        assert!(q.pop().is_none());
    }

    struct Fake;
    impl SpeechToText for Fake {
        fn transcribe(&mut self, samples: &[f32], language: Option<&str>) -> Result<Recognized> {
            Ok(Recognized {
                text: format!(" {} samples ", samples.len()),
                language: Some(language.unwrap_or("en").into()),
            })
        }
    }

    #[test]
    fn worker_turns_jobs_into_outputs_and_exits_when_done() {
        let (job_tx, job_rx) = crossbeam_channel::unbounded();
        let (out_tx, out_rx) = crossbeam_channel::unbounded();
        let handle = spawn_worker(|| Ok(Box::new(Fake)), job_rx, out_tx, || {});
        job_tx.send(job(Speaker::Others, 0, ChunkKind::Final, 1.5)).unwrap();
        drop(job_tx);
        handle.join().unwrap();
        let outputs: Vec<_> = out_rx.iter().collect();
        assert_eq!(outputs[0], Output::Ready);
        assert!(matches!(outputs[1], Output::Speed { .. }));
        match &outputs[2] {
            Output::Final { line: Some(line), .. } => {
                assert_eq!(line.text, "16000 samples");
                assert_eq!(line.speaker, Speaker::Others);
                assert_eq!(line.start, 1.5);
                assert_eq!(line.end, 2.5);
                assert_eq!(line.language.as_deref(), Some("en"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn load_failure_is_reported() {
        let (_job_tx, job_rx) = crossbeam_channel::unbounded::<Job>();
        let (out_tx, out_rx) = crossbeam_channel::unbounded();
        spawn_worker(|| anyhow::bail!("no model"), job_rx, out_tx, || {})
            .join()
            .unwrap();
        assert_eq!(out_rx.recv().unwrap(), Output::LoadFailed("no model".into()));
    }
}
