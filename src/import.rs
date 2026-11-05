//! Transcribing a recording someone already has (a Teams or Zoom video, a
//! voice memo). Its voices are told apart first, with speakrs (pyannote's
//! models, run in Rust on the processor), so each speaker's turns go to
//! Whisper on their own and every line has one speaker: "Speaker 1",
//! "Speaker 2", … in the order they first speak. The names are renamed in
//! the window.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result, anyhow, bail};
use chrono::NaiveDateTime;

use crate::audio::dsp::SAMPLE_RATE;
use crate::chunker::{Chunk, Chunker, ChunkerConfig};
use crate::meeting::{self, Meeting};
use crate::meetings;
use crate::models;
use crate::transcriber::Recognized;
use crate::transcript::{Line, Speaker, clean_text};

/// The speakrs release these files belong to, so a later change to the
/// repository can't break this build.
const SPEAKER_MODELS_URL: &str =
    "https://huggingface.co/avencera/speakrs-models/resolve/67f963b6a1d94fcd4678948237b3cc8bd69bf1d8";

/// What the processor (CPU) mode of speakrs loads, about 33 MB in all.
/// From pyannote's segmentation-3.0 and WeSpeaker's ResNet34, CC BY 4.0.
pub const SPEAKER_MODEL_FILES: &[&str] = &[
    "segmentation-3.0.safetensors",
    "wespeaker-multimask-tail.safetensors",
    "wespeaker-voxceleb-resnet34.min_num_samples.txt",
    "plda_lda.npy",
    "plda_tr.npy",
    "plda_mu.npy",
    "plda_psi.npy",
    "plda_phi.npy",
    "plda_mean1.npy",
    "plda_mean2.npy",
];

/// Speech this close to the next of the same voice is one turn.
const MERGE_GAP: f32 = 1.0;
/// Shorter pieces are left out of the turns: a cough, or the edge of a
/// word the model gave the wrong voice. Their sound stays in the turn
/// next to them thanks to `PAD`.
const MIN_PIECE: f32 = 0.3;
/// Sound kept either side of a turn, so first and last words are whole:
/// at most this, and at most half the gap to the next voice, so none of
/// theirs comes along.
const PAD: f32 = 0.2;

/// Where the speaker models live: next to the Whisper models.
pub fn speaker_models_dir() -> PathBuf {
    models::models_dir().join("speakers")
}

pub fn speaker_models_present() -> bool {
    let dir = speaker_models_dir();
    SPEAKER_MODEL_FILES.iter().all(|f| dir.join(f).is_file())
}

/// Downloads the speaker models that are missing. `progress` gets the
/// files done and the total.
pub fn download_speaker_models(cancel: Arc<AtomicBool>, mut progress: impl FnMut(usize, usize)) -> Result<()> {
    let dir = speaker_models_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("could not create {}", dir.display()))?;
    let total = SPEAKER_MODEL_FILES.len();
    for (i, name) in SPEAKER_MODEL_FILES.iter().enumerate() {
        progress(i, total);
        let path = dir.join(name);
        if path.is_file() {
            continue;
        }
        let part = dir.join(format!("{name}.part"));
        models::download_file(
            &format!("{SPEAKER_MODELS_URL}/{name}"),
            &part,
            cancel.clone(),
            |_, _| {},
        )?;
        std::fs::rename(&part, &path).with_context(|| format!("could not write {}", path.display()))?;
    }
    progress(total, total);
    Ok(())
}

/// A stretch of one voice, in seconds from the start of the recording.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Turn {
    pub start: f32,
    pub end: f32,
    /// 0 for the first voice heard, 1 for the next, …
    pub speaker: usize,
}

/// Tells the voices in 16 kHz mono `audio` apart.
pub fn find_speakers(audio: &[f32]) -> Result<Vec<Turn>> {
    let dir = speaker_models_dir();
    let mut pipeline = speakrs::OwnedDiarizationPipeline::from_dir(&dir, speakrs::ExecutionMode::Cpu)
        .map_err(|e| anyhow!("could not load the speaker models from {}: {e}", dir.display()))?;
    let result = pipeline
        .run(audio)
        .map_err(|e| anyhow!("could not tell the speakers apart: {e}"))?;
    let segments: Vec<(f32, f32, String)> = result
        .discrete_diarization
        .to_segments()
        .into_iter()
        .map(|s| (s.start as f32, s.end as f32, s.speaker))
        .collect();
    Ok(turns(&segments))
}

/// The model's pieces (start, end, voice label) as turns: voices numbered
/// by when they are first heard, short pieces dropped, a voice's pieces
/// joined across short gaps, someone talking over a turn left in it, and
/// a turn that starts before the last one ends split in the middle of the
/// overlap.
pub fn turns(segments: &[(f32, f32, String)]) -> Vec<Turn> {
    let mut pieces: Vec<&(f32, f32, String)> = segments.iter().filter(|s| s.1 - s.0 >= MIN_PIECE).collect();
    pieces.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut labels: Vec<&str> = Vec::new();
    let mut out: Vec<Turn> = Vec::new();
    for (start, end, label) in pieces {
        let speaker = labels.iter().position(|l| l == label).unwrap_or_else(|| {
            labels.push(label);
            labels.len() - 1
        });
        let (mut start, end) = (*start, *end);
        if let Some(last) = out.last_mut() {
            if last.speaker == speaker && start - last.end < MERGE_GAP {
                last.end = last.end.max(end);
                continue;
            }
            // Someone talking over the current speaker: a "yes" inside
            // their turn is heard in it anyway.
            if end <= last.end {
                continue;
            }
            if start < last.end {
                let middle = (start + last.end) / 2.0;
                last.end = middle;
                start = middle;
            }
        }
        out.push(Turn { start, end, speaker });
    }
    out
}

/// Pieces of speech for Whisper: each turn (or, with no turns, the whole
/// recording) split at pauses as a live meeting is, with the voice it
/// belongs to.
pub fn chunks(audio: &[f32], turns: Option<&[Turn]>) -> Vec<(Option<usize>, Chunk)> {
    let cfg = ChunkerConfig {
        partial_every: 0.0,
        ..Default::default()
    };
    let at = |seconds: f32| ((seconds.max(0.0) * SAMPLE_RATE as f32) as usize).min(audio.len());
    let whole = [Turn {
        start: 0.0,
        end: audio.len() as f32 / SAMPLE_RATE as f32,
        speaker: 0,
    }];
    let (list, named) = match turns {
        Some(t) => (t, true),
        None => (&whole[..], false),
    };
    let mut out = Vec::new();
    for (i, turn) in list.iter().enumerate() {
        let before = i.checked_sub(1).map_or(f32::INFINITY, |p| turn.start - list[p].end);
        let after = list.get(i + 1).map_or(f32::INFINITY, |n| n.start - turn.end);
        let pad = |gap: f32| PAD.min(gap.max(0.0) / 2.0);
        let (from, to) = (at(turn.start - pad(before)), at(turn.end + pad(after)));
        if from >= to {
            continue;
        }
        let offset = from as f32 / SAMPLE_RATE as f32;
        let mut chunker = Chunker::new(cfg.clone());
        let mut pieces = chunker.push(&audio[from..to]);
        pieces.extend(chunker.flush());
        for mut chunk in pieces {
            chunk.start += offset;
            out.push((named.then_some(turn.speaker), chunk));
        }
    }
    out
}

/// The name a voice is shown with until it is renamed.
pub fn speaker_name(speaker: usize) -> String {
    format!("Speaker {}", speaker + 1)
}

/// Where an import is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Stage {
    Reading { seconds: f32 },
    DownloadingModels { done: usize, total: usize },
    FindingSpeakers,
    Transcribing { done_seconds: f32, total_seconds: f32 },
}

/// What an import is doing, for the window and the command line.
pub fn stage_text(stage: &Stage) -> String {
    match *stage {
        Stage::Reading { seconds } => format!("Reading the recording… {}", crate::transcript::format_time(seconds)),
        Stage::DownloadingModels { done, total } => format!("Downloading the speaker models… {done} of {total}"),
        Stage::FindingSpeakers => "Telling the speakers apart…".into(),
        Stage::Transcribing {
            done_seconds,
            total_seconds,
        } => format!(
            "Transcribing… {}%",
            (100.0 * done_seconds / total_seconds.max(0.001)).round() as u32
        ),
    }
}

/// Turns a recording into transcript lines. `recognize` runs Whisper on
/// one piece; `stage` hears how far it has got.
pub fn transcribe_file(
    path: &Path,
    tell_speakers: bool,
    cancel: Arc<AtomicBool>,
    mut stage: impl FnMut(Stage),
    mut recognize: impl FnMut(&[f32]) -> Result<Recognized>,
) -> Result<(Vec<Line>, f32)> {
    let audio = crate::media::decode(path, &cancel, |seconds| stage(Stage::Reading { seconds }))?;
    let duration = audio.len() as f32 / SAMPLE_RATE as f32;
    let turns = if tell_speakers {
        if !speaker_models_present() {
            download_speaker_models(cancel.clone(), |done, total| {
                stage(Stage::DownloadingModels { done, total })
            })
            .context("could not download the speaker models")?;
        }
        stage(Stage::FindingSpeakers);
        Some(find_speakers(&audio)?)
    } else {
        None
    };
    let pieces = chunks(&audio, turns.as_deref());
    drop(audio);
    let total_seconds: f32 = pieces
        .iter()
        .map(|(_, c)| c.samples.len() as f32 / SAMPLE_RATE as f32)
        .sum();
    let mut done_seconds = 0.0;
    let mut lines = Vec::new();
    for (speaker, chunk) in pieces {
        if cancel.load(Ordering::Relaxed) {
            bail!("cancelled");
        }
        stage(Stage::Transcribing {
            done_seconds,
            total_seconds,
        });
        let recognized = recognize(&chunk.samples)?;
        done_seconds += chunk.samples.len() as f32 / SAMPLE_RATE as f32;
        if let Some(text) = clean_text(&recognized.text) {
            lines.push(Line {
                speaker: Speaker::Others,
                start: chunk.start,
                end: chunk.end(),
                text,
                language: recognized.language,
                who: speaker.map(speaker_name),
            });
        }
    }
    Ok((lines, duration))
}

/// The meeting for an imported recording: named after the file, dated
/// when it started (its file was last written when it ended).
pub fn meeting_for(path: &Path, lines: Vec<Line>, duration: f32) -> Meeting {
    let title = path
        .file_stem()
        .map(|s| meeting::one_line(&s.to_string_lossy()))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "Recording".into());
    let date = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(meetings::local_time)
        .map(|end| end - chrono::Duration::seconds(duration as i64))
        .unwrap_or_else(|_| chrono::Local::now().naive_local());
    let mut m = Meeting::new(&title, start_of_minute(date));
    m.duration = duration.ceil() as u64;
    m.languages = meeting::merge_languages(&[], &lines);
    m.lines = lines;
    let source = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    m.extra_frontmatter
        .push(format!("source: {}", meeting::yaml_quote(&source)));
    m
}

fn start_of_minute(date: NaiveDateTime) -> NaiveDateTime {
    use chrono::Timelike as _;
    date.with_second(0).and_then(|d| d.with_nanosecond(0)).unwrap_or(date)
}

/// Writes `m` as a new file in `dir`; never replaces one.
pub fn save_new(dir: &Path, m: &Meeting) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    let path = meetings::unused_path(dir, &meetings::file_stem(m.date, &m.title), "md", None);
    std::fs::write(&path, m.to_markdown()).with_context(|| format!("could not write {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start: f32, end: f32, label: &str) -> (f32, f32, String) {
        (start, end, label.to_string())
    }

    #[test]
    fn voices_are_numbered_in_the_order_they_speak() {
        let t = turns(&[
            seg(0.0, 2.0, "SPEAKER_03"),
            seg(2.5, 4.0, "SPEAKER_00"),
            seg(4.5, 6.0, "SPEAKER_03"),
        ]);
        let speakers: Vec<_> = t.iter().map(|t| t.speaker).collect();
        assert_eq!(speakers, [0, 1, 0]);
        assert_eq!(speaker_name(t[1].speaker), "Speaker 2");
    }

    #[test]
    fn a_voice_is_joined_across_short_gaps_only() {
        let t = turns(&[seg(0.0, 2.0, "A"), seg(2.5, 4.0, "A"), seg(6.0, 7.0, "A")]);
        assert_eq!(
            t,
            [
                Turn {
                    start: 0.0,
                    end: 4.0,
                    speaker: 0
                },
                Turn {
                    start: 6.0,
                    end: 7.0,
                    speaker: 0
                }
            ]
        );
    }

    #[test]
    fn short_pieces_and_talking_over_are_left_to_the_speaker() {
        let t = turns(&[
            seg(0.0, 5.0, "A"),
            // "Yes" while A talks.
            seg(2.0, 2.8, "B"),
            // A blip of the wrong voice.
            seg(5.0, 5.1, "B"),
            // B starts before A has quite finished.
            seg(4.6, 8.0, "B"),
        ]);
        assert_eq!(
            t,
            [
                Turn {
                    start: 0.0,
                    end: 4.8,
                    speaker: 0
                },
                Turn {
                    start: 4.8,
                    end: 8.0,
                    speaker: 1
                }
            ]
        );
    }

    fn tone(seconds: f32) -> Vec<f32> {
        (0..(seconds * SAMPLE_RATE as f32) as usize)
            .map(|i| (i as f32 * 0.05).sin() * 0.3)
            .collect()
    }

    #[test]
    fn each_turn_is_its_own_chunk_with_its_own_voice() {
        let mut audio = tone(3.0);
        audio.extend(tone(5.0));
        let turns = [
            Turn {
                start: 0.0,
                end: 3.0,
                speaker: 0,
            },
            Turn {
                start: 3.0,
                end: 5.0,
                speaker: 1,
            },
            Turn {
                start: 6.0,
                end: 8.0,
                speaker: 0,
            },
        ];
        let c = chunks(&audio, Some(&turns));
        assert_eq!(
            c.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            [Some(0), Some(1), Some(0)]
        );
        assert!(c[0].1.start.abs() < 0.01);
        // Back to back: no sound of the other voice.
        assert!((c[1].1.start - 3.0).abs() < 0.01, "{}", c[1].1.start);
        // After a gap: a little before the turn.
        assert!((c[2].1.start - (6.0 - PAD)).abs() < 0.01, "{}", c[2].1.start);
        // Without turns, nobody is named.
        assert!(chunks(&audio, None).iter().all(|(s, _)| s.is_none()));
    }

    #[test]
    fn an_imported_meeting_is_named_after_its_file() {
        let line = Line {
            speaker: Speaker::Others,
            start: 1.0,
            end: 2.0,
            text: "Hello".into(),
            language: Some("en".into()),
            who: Some("Speaker 1".into()),
        };
        let m = meeting_for(Path::new("/nowhere/Weekly sync.mp4"), vec![line], 61.2);
        assert_eq!(m.title, "Weekly sync");
        assert_eq!(m.duration, 62);
        assert_eq!(m.languages, ["en"]);
        assert_eq!(m.extra_frontmatter, ["source: \"Weekly sync.mp4\""]);
        let text = m.to_markdown();
        assert!(text.contains("**[00:01] Speaker 1:** Hello"), "{text}");
        let back = Meeting::parse(&text, "x", m.date);
        assert_eq!(back.lines[0].label(), "Speaker 1");
    }
}
