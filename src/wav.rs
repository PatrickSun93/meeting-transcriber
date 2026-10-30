//! Offline mode: runs WAV files through the same pipeline as live audio.
//! Used for testing and for transcribing recordings.

use std::path::Path;

use anyhow::{Context as _, Result};

use crate::audio::AudioPacket;
use crate::audio::dsp::{Resampler, SAMPLE_RATE, interleaved_to_mono};
use crate::chunker::ChunkerConfig;
use crate::session::{Router, SessionConfig};
use crate::transcriber::{Job, JobQueue, Output, SpeechToText, run_job};
use crate::transcript::{Line, Speaker, Transcript};

/// Reads a WAV file as 16 kHz mono.
pub fn read_wav(path: &Path) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(path).with_context(|| format!("could not open {}", path.display()))?;
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / scale))
                .collect::<Result<_, _>>()?
        }
    };
    let mono = interleaved_to_mono(&samples, spec.channels as usize);
    Ok(Resampler::new(spec.sample_rate).process(&mono))
}

/// Feeds both tracks through the live pipeline in 100 ms packets, as if they
/// were being captured, then transcribes every final chunk.
pub fn transcribe_tracks(
    engine: &mut dyn SpeechToText,
    me: Option<&[f32]>,
    others: Option<&[f32]>,
    language: Option<&str>,
    mut on_line: impl FnMut(&Line),
) -> Result<Transcript> {
    let cfg = SessionConfig {
        capture_me: me.is_some(),
        capture_others: others.is_some(),
        chunker: ChunkerConfig {
            partial_every: 0.0,
            ..Default::default()
        },
        time_offset: 0.0,
        first_utterance: 0,
        language: language.map(str::to_string),
    };
    let mut router = Router::new(&cfg);
    let mut jobs: Vec<Job> = Vec::new();
    let packet = (SAMPLE_RATE / 10) as usize;
    let tracks = [
        (Speaker::Me, me.unwrap_or(&[])),
        (Speaker::Others, others.unwrap_or(&[])),
    ];
    let longest = tracks.iter().map(|(_, t)| t.len()).max().unwrap_or(0);
    let mut pos = 0;
    while pos < longest {
        for (speaker, track) in tracks {
            if pos < track.len() {
                let end = (pos + packet).min(track.len());
                let p = AudioPacket {
                    speaker,
                    samples: track[pos..end].to_vec(),
                };
                jobs.extend(router.push(p, end as f32 / SAMPLE_RATE as f32));
            }
        }
        pos += packet;
    }
    jobs.extend(router.flush());

    let mut queue = JobQueue::default();
    for job in jobs {
        queue.push(job);
    }
    // `run_job` reports timing on this channel; offline mode does not need it.
    let (speed_tx, _speed_rx) = crossbeam_channel::unbounded();
    let mut transcript = Transcript::default();
    while let Some(job) = queue.pop() {
        match run_job(engine, job, &speed_tx) {
            Output::Final {
                line: Some(line),
                utterance,
                ..
            } => {
                on_line(&line);
                transcript.add_final(utterance, line);
            }
            Output::Error(e) => eprintln!("error: {e}"),
            _ => {}
        }
    }
    Ok(transcript)
}
