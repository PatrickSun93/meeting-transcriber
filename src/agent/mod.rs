//! An experimental voice agent: talk to a language model and it talks
//! back, and stops when you talk over it. `meeting-transcriber agent`.
//!
//! ```text
//! microphone ─▶ chunker ─▶ Whisper ─▶ LLM (streamed) ─▶ sentences ─▶ say ─▶ speakers
//!     │          (pauses)    (worker)    (turn thread)                (TTS thread)  (player)
//!     └─▶ barge-in: loud frames while the agent thinks or speaks ─▶ stop all of it
//! ```
//!
//! Everything comes together in one loop (`Agent::run`) that owns the
//! conversation: the history, who has the floor (`floor`), the record
//! (`record`). The slow parts run on their own threads and report back on
//! channels, tagged with the turn they belong to, so whatever a cancelled
//! turn still sends is recognised and dropped.
//!
//! The scenario (a clinic's front desk, `clinic`) and its data are a demo.

pub mod clinic;
pub mod floor;
pub mod llm;
pub mod player;
pub mod record;
pub mod sentence;
pub mod tts;

use std::io::{BufRead as _, IsTerminal as _, Write as _};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context as _, Result, bail};
use crossbeam_channel::{Receiver, Sender, select};

use crate::audio::dsp::{SAMPLE_RATE, rms};
use crate::audio::{self, AudioPacket};
use crate::chunker::{Chunk, ChunkKind, Chunker, ChunkerConfig};
use crate::models::{self, DEFAULT_MODEL, ModelInfo};
use crate::settings::Settings;
use crate::transcriber::{SpeechToText, WhisperEngine};
use crate::transcript::clean_text;
use clinic::{Clinic, TextReply, ToolMode};
use floor::{Action, BargeInConfig, FRAME_SECONDS, Floor, Phase};
use llm::{Api, LlmConfig, Message, StreamEvent, ToolCall, ToolSpec};
use player::{Clock, Output, PlayEvent, Segment, Speakers};
use record::{AGENT, CALLER, Latency, Marks, Record, TOOL, TurnStats};
use sentence::SentenceSplitter;
use tts::{Say, Synth};

/// The `agent` part of the program's usage text.
#[macro_export]
macro_rules! agent_usage {
    () => {
        "\
  meeting-transcriber agent [--api ollama|openai|anthropic] [--url URL]
                            [--model NAME] [--whisper PATH_OR_ID]
                            [--language CODE|auto] [--voice NAME] [--rate WPM]
                            [--tools native|json] [--pause SECONDS]
                            [--barge-in-level RMS] [--barge-in-ms MS]
                            [--no-barge-in] [--no-partials] [--out DIR]
      EXPERIMENTAL DEMO (macOS): a voice agent for a made-up clinic's
      front desk. Speak, and a language model answers aloud; talk over
      it to interrupt. Use headphones. Defaults: Ollama on this computer
      with gemma4:12b, the Whisper model chosen in Settings, English, the
      voice Samantha. Prints both sides and each turn's latency, and saves
      the call as Markdown in the meetings folder's \"Voice Agent\" folder.
      --no-barge-in ignores the microphone while the agent speaks (for
      speakers instead of headphones). Type q and Enter to hang up.
      For testing: --input FILE plays a WAV recording (16 kHz mono) as
      the caller; --no-sound times the voice without playing it.
      --full-window runs Whisper on its usual 30 s window (slower).
"
    };
}

pub const USAGE: &str = agent_usage!();

/// Said when the model fails, so the caller is not left in silence.
const SORRY: &str = "Sorry, I'm having trouble right now. Could you say that again?";
/// Steers Whisper's spelling.
const WHISPER_PROMPT: &str = "A phone call to Maple Street Clinic to book an appointment.";
/// Tool rounds in one turn before giving up.
const MAX_ROUNDS: usize = 4;

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub llm: LlmConfig,
    pub whisper: PathBuf,
    pub language: Option<String>,
    pub voice: Option<String>,
    pub words_per_minute: Option<u32>,
    pub tools: ToolMode,
    pub chunker: ChunkerConfig,
    pub barge_in: BargeInConfig,
    pub partials: bool,
    pub out_dir: PathBuf,
    /// A recording played as the caller instead of the microphone.
    pub input: Option<PathBuf>,
    /// False: the voice is timed but not played (for tests).
    pub sound: bool,
    /// Whisper's encoder looks at the clip only, not 30 seconds.
    pub short_window: bool,
}

/// `meeting-transcriber agent …`
pub fn run_cli(args: &[String]) -> Result<()> {
    let cfg = parse_args(args)?;
    Agent::start(cfg)?.run()
}

fn parse_args(args: &[String]) -> Result<AgentConfig> {
    let settings = Settings::load();
    models::set_configured_dir(settings.models_dir_path());
    models::set_move_source(settings.models_move_from_path());

    let mut api = Api::Ollama;
    let mut url = None;
    let mut model = None;
    let mut whisper = None;
    let mut language = Some("en".to_string());
    let mut voice = Some("Samantha".to_string());
    let mut wpm = None;
    let mut tools = ToolMode::Native;
    let mut chunker = ChunkerConfig {
        // A short "Yes." ends at the pause too.
        min_seconds: 0.8,
        max_seconds: 15.0,
        pause_seconds: 0.6,
        partial_every: 1.0,
        ..ChunkerConfig::default()
    };
    let mut barge_in = BargeInConfig::default();
    let mut partials = true;
    let mut out_dir = None;
    let mut input = None;
    let mut sound = true;
    let mut short_window = true;

    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let mut value = || {
            it.next()
                .with_context(|| format!("{flag} needs a value\n\n{USAGE}"))
                .cloned()
        };
        match flag.as_str() {
            "--api" => {
                let v = value()?;
                api = Api::from_id(&v).with_context(|| format!("--api is ollama, openai or anthropic, not {v:?}"))?;
            }
            "--url" => url = Some(value()?),
            "--model" => model = Some(value()?),
            "--whisper" => whisper = Some(value()?),
            "--language" => {
                let v = value()?;
                language = (v != "auto").then_some(v);
            }
            "--voice" => voice = Some(value()?).filter(|v| v != "default"),
            "--rate" => wpm = Some(value()?.parse().context("--rate takes words per minute, e.g. 190")?),
            "--tools" => {
                tools = match value()?.as_str() {
                    "native" => ToolMode::Native,
                    "json" => ToolMode::Json,
                    other => bail!("--tools is native or json, not {other:?}"),
                }
            }
            "--pause" => {
                let v: f32 = value()?.parse().context("--pause takes seconds, e.g. 0.6")?;
                if !(0.2..=3.0).contains(&v) {
                    bail!("--pause is between 0.2 and 3 seconds");
                }
                chunker.pause_seconds = v;
                chunker.min_seconds = chunker.min_seconds.max(v + 0.2);
            }
            "--barge-in-level" => {
                barge_in.level = value()?
                    .parse()
                    .context("--barge-in-level takes an RMS level, e.g. 0.02")?
            }
            "--barge-in-ms" => {
                let v: f32 = value()?.parse().context("--barge-in-ms takes milliseconds, e.g. 200")?;
                barge_in.frames = ((v / 1000.0 / FRAME_SECONDS).round() as usize).max(1);
            }
            "--no-barge-in" => barge_in.enabled = false,
            "--no-partials" => partials = false,
            "--out" => out_dir = Some(PathBuf::from(value()?)),
            "--input" => input = Some(PathBuf::from(value()?)),
            "--no-sound" => sound = false,
            "--full-window" => short_window = false,
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            other => bail!("unknown option {other:?}\n\n{USAGE}"),
        }
    }
    if !partials {
        chunker.partial_every = 0.0;
    }

    let api_key = api.key_env().and_then(|k| std::env::var(k).ok()).unwrap_or_default();
    if api == Api::Anthropic && api_key.trim().is_empty() {
        bail!("--api anthropic needs the key in ANTHROPIC_API_KEY");
    }
    let model = model.unwrap_or_else(|| LlmConfig::default_model(api).to_string());
    if model.is_empty() {
        bail!("--api {} needs --model", api.id());
    }
    let whisper = match whisper {
        Some(m) => ModelInfo::by_id(&m).map_or_else(|| PathBuf::from(&m), |info| info.path()),
        None => ModelInfo::by_id(&settings.model).unwrap_or(DEFAULT_MODEL).path(),
    };
    if !whisper.is_file() {
        bail!(
            "Whisper model not found at {}. Download one in the app's Settings, or pass --whisper /path/to/ggml-model.bin",
            whisper.display()
        );
    }
    Ok(AgentConfig {
        llm: LlmConfig {
            api,
            url: url.unwrap_or_else(|| LlmConfig::default_url(api).to_string()),
            model,
            api_key,
        },
        whisper,
        language,
        voice,
        words_per_minute: wpm,
        tools,
        chunker,
        barge_in,
        partials,
        out_dir: out_dir.unwrap_or_else(|| settings.meetings_dir_path().join("Voice Agent")),
        input,
        sound,
        short_window,
    })
}

/// Something heard, written down.
#[derive(Debug)]
struct Heard {
    kind: ChunkKind,
    /// None: nothing (or only noise) was said.
    text: Option<String>,
    /// When the chunker sent it.
    sent: Instant,
}

struct AsrJob {
    chunk: Chunk,
    sent: Instant,
}

/// What a turn's thread reports.
#[derive(Debug)]
enum TurnEvent {
    FirstToken(Instant),
    Sentence(String),
    /// Tools were used: the messages to add to the history, and for the
    /// terminal and the record, each call with its result.
    Tools {
        history: Vec<Message>,
        calls: Vec<(ToolCall, String)>,
    },
    Done,
    Failed(String),
}

enum Event {
    Heard(Heard),
    Turn { turn: u64, event: TurnEvent },
    SpeakFailed { turn: u64, index: usize, error: String },
    Quit,
}

/// The turn being answered.
struct Live {
    turn: u64,
    /// None for the greeting.
    marks: Option<Marks>,
    caller: String,
    cancel: Arc<AtomicBool>,
    sentences: Vec<String>,
    /// Sentences played to the end (or that failed to be made).
    done: usize,
    llm_done: bool,
    tools: Vec<String>,
    /// When the agent started speaking, for the record.
    spoke_at: Option<f32>,
}

impl Live {
    fn new(turn: u64, marks: Option<Marks>, caller: String) -> Self {
        Self {
            turn,
            marks,
            caller,
            cancel: Arc::new(AtomicBool::new(false)),
            sentences: Vec::new(),
            done: 0,
            llm_done: false,
            tools: Vec::new(),
            spoke_at: None,
        }
    }

    fn finished(&self) -> bool {
        self.llm_done && self.done >= self.sentences.len()
    }
}

struct Agent {
    cfg: AgentConfig,
    events_tx: Sender<Event>,
    events: Receiver<Event>,
    audio: Receiver<AudioPacket>,
    played: Receiver<PlayEvent>,
    asr: Sender<AsrJob>,
    /// Joined before the program ends: Whisper's GPU state must be freed
    /// before the process exits, or Metal aborts.
    asr_thread: Option<std::thread::JoinHandle<()>>,
    speak: Sender<(u64, usize, String)>,
    /// The turn whose sentences the voice thread should still make.
    voice_turn: Arc<AtomicU64>,
    output: Arc<dyn Output>,
    clinic: Arc<Mutex<Clinic>>,
    tools: Vec<ToolSpec>,
    history: Vec<Message>,
    floor: Floor,
    chunker: Chunker,
    frame: Vec<f32>,
    live: Option<Live>,
    record: Record,
    t0: Instant,
    /// A partial transcript is shown on the last terminal line.
    partial_shown: bool,
    _mic: audio::Capture,
}

impl Agent {
    fn start(cfg: AgentConfig) -> Result<Self> {
        eprintln!("Voice agent (experimental demo). Use headphones: the agent must not hear itself.");
        let synth = Say::new(cfg.voice.clone(), cfg.words_per_minute)?;
        let voice_name = synth.name();

        let loading = Instant::now();
        let mut engine = WhisperEngine::load(&cfg.whisper)?;
        engine.set_short_clips(cfg.short_window);
        // The first run on the GPU is slow (shaders); not in the first turn.
        let _ = engine.transcribe(&[0.0; SAMPLE_RATE as usize], cfg.language.as_deref());
        eprintln!(
            "Whisper: {} (loaded in {:.1}s)",
            cfg.whisper.file_name().unwrap_or_default().to_string_lossy(),
            loading.elapsed().as_secs_f32()
        );

        let today = chrono::Local::now().date_naive();
        let clinic = Clinic::demo(today);
        let system = clinic.prompt(cfg.tools);
        let tools = if cfg.tools == ToolMode::Native {
            Clinic::tools()
        } else {
            Vec::new()
        };
        let history = vec![Message::system(&system)];

        let warming = Instant::now();
        warm_up(&cfg.llm, &history, &tools).with_context(|| {
            format!(
                "the language model ({} at {}) did not answer",
                cfg.llm.model, cfg.llm.url
            )
        })?;
        eprintln!(
            "LLM: {} {} at {} (ready in {:.1}s)",
            cfg.llm.api.id(),
            cfg.llm.model,
            cfg.llm.url,
            warming.elapsed().as_secs_f32()
        );

        let (events_tx, events) = crossbeam_channel::unbounded();
        let (played_tx, played) = crossbeam_channel::unbounded();
        let output: Arc<dyn Output> = if cfg.sound {
            let speakers = Speakers::open(played_tx)?;
            eprintln!(
                "Voice: {voice_name}, through {} at {} Hz",
                speakers.name,
                speakers.rate()
            );
            Arc::new(speakers)
        } else {
            eprintln!("Voice: {voice_name}, not played (--no-sound)");
            Arc::new(Clock::start(SAMPLE_RATE, played_tx))
        };

        let (asr, asr_thread) = start_asr(engine, cfg.language.clone(), events_tx.clone())?;
        let voice_turn = Arc::new(AtomicU64::new(0));
        let speak = start_voice(Box::new(synth), output.clone(), voice_turn.clone(), events_tx.clone())?;

        let (audio_tx, audio) = crossbeam_channel::unbounded();
        let mic = match &cfg.input {
            Some(path) => audio::start_file(path.clone(), crate::transcript::Speaker::Me, audio_tx)?,
            None => audio::start_mic(audio_tx)?,
        };
        eprintln!("Caller: {}", mic.name);

        let setup = vec![
            format!(
                "Speech to text: Whisper {}",
                cfg.whisper.file_name().unwrap_or_default().to_string_lossy()
            ),
            format!(
                "LLM: {} {} ({} tools)",
                cfg.llm.api.id(),
                cfg.llm.model,
                if cfg.tools == ToolMode::Native {
                    "native"
                } else {
                    "JSON"
                }
            ),
            format!("Text to speech: {voice_name}"),
            format!(
                "End of speech: {:.1} s pause; barge-in: {}",
                cfg.chunker.pause_seconds,
                if cfg.barge_in.enabled {
                    format!(
                        "{:.0} ms above RMS {}",
                        cfg.barge_in.frames as f32 * FRAME_SECONDS * 1000.0,
                        cfg.barge_in.level
                    )
                } else {
                    "off".into()
                }
            ),
        ];
        let t0 = Instant::now();
        Ok(Self {
            floor: Floor::new(cfg.barge_in),
            chunker: Chunker::new(cfg.chunker.clone()),
            record: Record::new(chrono::Local::now().naive_local(), t0, setup),
            cfg,
            events_tx,
            events,
            audio,
            played,
            asr,
            asr_thread: Some(asr_thread),
            speak,
            voice_turn,
            output,
            clinic: Arc::new(Mutex::new(clinic)),
            tools,
            history,
            frame: Vec::new(),
            live: None,
            t0,
            partial_shown: false,
            _mic: mic,
        })
    }

    fn run(mut self) -> Result<()> {
        // q and Enter hangs up.
        let quit = self.events_tx.clone();
        let interactive = std::io::stdin().is_terminal();
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines() {
                match line {
                    Ok(l) if matches!(l.trim(), "q" | "quit" | "exit") => {
                        let _ = quit.send(Event::Quit);
                        return;
                    }
                    Ok(_) => {}
                    Err(_) => return,
                }
            }
        });
        eprintln!(
            "Saving the call to {}{}\n",
            self.cfg.out_dir.display(),
            if interactive {
                ". Type q and Enter to hang up."
            } else {
                ""
            }
        );

        // The agent picks up.
        let turn = self.floor.agent_starts();
        let mut live = Live::new(turn, None, String::new());
        live.llm_done = true;
        self.live = Some(live);
        self.voice_turn.store(turn, Ordering::Relaxed);
        self.add_sentence(turn, clinic::GREETING.to_string());

        let tick = crossbeam_channel::tick(std::time::Duration::from_millis(200));
        // A recording as the caller: when it has ended and the agent has
        // nothing left to say, the call is over.
        let mut input_ended: Option<Instant> = None;
        loop {
            select! {
                recv(self.audio) -> packet => match packet {
                    Ok(p) => self.on_audio(&p.samples),
                    Err(_) if self.cfg.input.is_some() => {
                        for chunk in self.chunker.flush() {
                            let _ = self.asr.send(AsrJob { chunk, sent: Instant::now() });
                        }
                        self.audio = crossbeam_channel::never();
                        input_ended = Some(Instant::now());
                    }
                    Err(_) => bail!("the microphone stopped"),
                },
                recv(tick) -> _ => {
                    if let Some(at) = input_ended {
                        if self.live.is_some() {
                            input_ended = Some(Instant::now());
                        } else if at.elapsed().as_secs_f32() > 5.0 {
                            break;
                        }
                    }
                }
                recv(self.played) -> e => {
                    if let Ok(e) = e {
                        self.on_played(e);
                    }
                }
                recv(self.events) -> e => match e {
                    Ok(Event::Heard(h)) => self.on_heard(h),
                    Ok(Event::Turn { turn, event }) => self.on_turn(turn, event),
                    Ok(Event::SpeakFailed { turn, index, error }) => {
                        self.line(&format!("  (voice failed: {error})"));
                        self.sentence_done(turn, index);
                    }
                    Ok(Event::Quit) | Err(_) => break,
                },
            }
        }
        if let Some(live) = &self.live {
            let turn = live.turn;
            self.interrupt(turn);
        }
        self.save();
        self.summary();
        drop(std::mem::replace(&mut self.asr, crossbeam_channel::unbounded().0));
        if let Some(t) = self.asr_thread.take() {
            let _ = t.join();
        }
        Ok(())
    }

    fn duration(&self) -> u64 {
        self.t0.elapsed().as_secs()
    }

    fn save(&mut self) {
        self.record.bookings = self.clinic.lock().map(|c| c.booking_lines()).unwrap_or_default();
        let duration = self.duration();
        if let Err(e) = self.record.save(&self.cfg.out_dir, duration) {
            self.line(&format!("Could not save the call: {e:#}"));
        }
    }

    fn summary(&mut self) {
        let turns = self.record.turns();
        let m = |f: fn(&Latency) -> Option<u32>| record::median(turns.iter().filter_map(|t| f(&t.latency)));
        let show = |v: Option<u32>| v.map_or("–".into(), |v| format!("{v} ms"));
        eprintln!(
            "\n{} turns. Median: ASR {}, LLM first token {}, first sentence {}, first sound {}.",
            turns.len(),
            show(m(|l| Some(l.asr))),
            show(m(|l| l.first_token)),
            show(m(|l| l.first_sentence)),
            show(m(|l| l.first_sound)),
        );
        let bookings = self.record.bookings.clone();
        for b in bookings {
            eprintln!("Booked (demo): {b}");
        }
        let path = self.record.save(&self.cfg.out_dir, self.duration());
        if let Ok(path) = path {
            eprintln!("Saved {}", path.display());
        }
    }

    /// Prints a line, over the partial transcript if one is shown.
    fn line(&mut self, text: &str) {
        let mut err = std::io::stderr().lock();
        if self.partial_shown {
            let _ = write!(err, "\r\x1b[2K");
            self.partial_shown = false;
        }
        let _ = writeln!(err, "{text}");
    }

    fn show_partial(&mut self, text: &str) {
        let mut err = std::io::stderr().lock();
        let width = 100;
        let mut shown: String = text
            .chars()
            .rev()
            .take(width)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        if text.chars().count() > width {
            shown.insert(0, '…');
        }
        let _ = write!(err, "\r\x1b[2K  … {shown}");
        let _ = err.flush();
        self.partial_shown = true;
    }

    fn on_audio(&mut self, samples: &[f32]) {
        let frame_len = (SAMPLE_RATE as f32 * FRAME_SECONDS) as usize;
        let mut i = 0;
        while i < samples.len() {
            let take = (frame_len - self.frame.len()).min(samples.len() - i);
            self.frame.extend_from_slice(&samples[i..i + take]);
            i += take;
            if self.frame.len() == frame_len {
                let level = rms(&self.frame);
                self.frame.clear();
                if let Some(Action::Interrupt { turn }) = self.floor.mic_frame(level) {
                    self.line("  [caller is talking: agent stops]");
                    self.interrupt(turn);
                }
            }
        }
        // Without barge-in, the agent does not listen while it has the floor
        // (half duplex): its own voice from speakers is not taken for the caller.
        if !self.cfg.barge_in.enabled && self.floor.phase() != Phase::Listening {
            self.chunker = Chunker::with_first_utterance(self.cfg.chunker.clone(), self.next_utterance());
            return;
        }
        let sent = Instant::now();
        for chunk in self.chunker.push(samples) {
            let _ = self.asr.send(AsrJob { chunk, sent });
        }
    }

    fn next_utterance(&self) -> u64 {
        // Utterance numbers only need to grow; the turn count is a fine source.
        (self.floor.turn() + 1) * 1000
    }

    fn on_heard(&mut self, h: Heard) {
        match h.kind {
            ChunkKind::Partial => {
                if let Some(text) = h.text {
                    self.show_partial(&text);
                }
            }
            ChunkKind::Final => {
                let Some(text) = h.text else {
                    if self.partial_shown {
                        self.line("  (nothing understood)");
                    }
                    return;
                };
                let asr = Instant::now();
                let at = self.record.at(h.sent) - self.cfg.chunker.pause_seconds;
                self.line(&format!("Caller: {text}"));
                self.record.say(CALLER, at.max(0.0), &text);
                let Action::StartTurn { turn, cancel } = self.floor.caller_said() else {
                    return;
                };
                if let Some(old) = cancel {
                    self.interrupt(old);
                }
                self.history.push(Message::user(&text));
                let marks = Marks {
                    ended: h.sent,
                    asr,
                    first_token: None,
                    first_sentence: None,
                    first_sound: None,
                };
                let live = Live::new(turn, Some(marks), text);
                let cancel = live.cancel.clone();
                self.live = Some(live);
                self.voice_turn.store(turn, Ordering::Relaxed);
                let ctx = TurnContext {
                    llm: self.cfg.llm.clone(),
                    tools: self.tools.clone(),
                    clinic: self.clinic.clone(),
                };
                let messages = self.history.clone();
                let tx = self.events_tx.clone();
                let _ = std::thread::Builder::new()
                    .name(format!("turn {turn}"))
                    .spawn(move || run_turn(turn, ctx, messages, cancel, tx));
            }
        }
    }

    fn live_turn(&mut self, turn: u64) -> Option<&mut Live> {
        self.live.as_mut().filter(|l| l.turn == turn)
    }

    fn on_turn(&mut self, turn: u64, event: TurnEvent) {
        // What tools did stays done (a booking is a booking), even for a
        // turn that was cut off since.
        if let TurnEvent::Tools { history, calls } = &event {
            self.history.extend(history.iter().cloned());
            let at = self.record.at(Instant::now());
            for (call, result) in calls {
                let text = format!("{} {} → {}", call.name, call.arguments, result);
                self.line(&format!("  (tool) {text}"));
                self.record.say(TOOL, at, &text);
            }
            if let Some(live) = self.live_turn(turn) {
                live.tools.extend(calls.iter().map(|(c, _)| c.name.clone()));
            }
            return;
        }
        if self.live_turn(turn).is_none() {
            return;
        }
        match event {
            TurnEvent::FirstToken(at) => {
                if let Some(m) = self.live_turn(turn).and_then(|l| l.marks.as_mut()) {
                    m.first_token.get_or_insert(at);
                }
            }
            TurnEvent::Sentence(s) => self.add_sentence(turn, s),
            TurnEvent::Done => {
                let live = self.live_turn(turn).expect("checked above");
                live.llm_done = true;
                if live.sentences.is_empty() {
                    live.llm_done = false;
                    self.on_turn(turn, TurnEvent::Failed("the model answered nothing".into()));
                    return;
                }
                self.maybe_finish(turn);
            }
            TurnEvent::Failed(error) => {
                self.line(&format!("  (LLM failed: {error})"));
                self.add_sentence(turn, SORRY.to_string());
                if let Some(live) = self.live_turn(turn) {
                    live.llm_done = true;
                }
                self.maybe_finish(turn);
            }
            TurnEvent::Tools { .. } => unreachable!(),
        }
    }

    fn add_sentence(&mut self, turn: u64, s: String) {
        let Some(live) = self.live_turn(turn) else {
            return;
        };
        let index = live.sentences.len();
        live.sentences.push(s.clone());
        if let Some(m) = live.marks.as_mut() {
            m.first_sentence.get_or_insert_with(Instant::now);
        }
        let _ = self.speak.send((turn, index, s));
    }

    fn on_played(&mut self, e: PlayEvent) {
        match e {
            PlayEvent::Started { turn, index, at } => {
                let t = self.record.at(at);
                let Some(live) = self.live_turn(turn) else {
                    return;
                };
                let text = live.sentences.get(index).cloned().unwrap_or_default();
                if index == 0 {
                    live.spoke_at = Some(t);
                    if let Some(m) = live.marks.as_mut() {
                        m.first_sound.get_or_insert(at);
                    }
                }
                self.floor.voice_started(turn);
                let prefix = if index == 0 { "Agent:" } else { "      " };
                self.line(&format!("{prefix} {text}"));
            }
            PlayEvent::Finished { turn, index } => self.sentence_done(turn, index),
        }
    }

    fn sentence_done(&mut self, turn: u64, _index: usize) {
        if let Some(live) = self.live_turn(turn) {
            live.done += 1;
        }
        self.maybe_finish(turn);
    }

    fn maybe_finish(&mut self, turn: u64) {
        if !self.live_turn(turn).is_some_and(|l| l.finished()) {
            return;
        }
        let live = self.live.take().expect("checked above");
        let said = live.sentences.join(" ");
        self.end_turn(live, said, false);
        self.floor.turn_done(turn);
    }

    /// Stops `turn`: its model, its voice, and what it was about to say.
    fn interrupt(&mut self, turn: u64) {
        let Some(live) = self.live.take_if(|l| l.turn == turn) else {
            return;
        };
        live.cancel.store(true, Ordering::Relaxed);
        self.voice_turn.store(0, Ordering::Relaxed);
        let cut = self.output.stop();
        // Only what was heard goes into the history: the sentences played
        // to the end, and the part of the one that was cut off.
        let mut said: Vec<String> = live.sentences.iter().take(live.done).cloned().collect();
        if let Some(cut) = cut.filter(|c| c.turn == turn)
            && let Some(s) = live.sentences.get(cut.index)
        {
            let words: Vec<&str> = s.split_whitespace().collect();
            let heard = ((words.len() as f32) * cut.played).floor() as usize;
            if heard > 0 {
                said.push(format!("{}…", words[..heard].join(" ")));
            }
        }
        let said = said.join(" ");
        self.end_turn(live, said, true);
    }

    fn end_turn(&mut self, live: Live, said: String, interrupted: bool) {
        let said = said.trim().to_string();
        if !said.is_empty() {
            let text = if interrupted {
                format!("{said} [interrupted]")
            } else {
                said.clone()
            };
            let at = live.spoke_at.unwrap_or_else(|| self.record.at(Instant::now()));
            self.record.say(AGENT, at, &text);
            self.history.push(Message::assistant(&said));
        }
        if let Some(marks) = &live.marks {
            let latency = marks.latency();
            self.line(&format!("  ⏱ {}", latency.summary()));
            self.record.turn(TurnStats {
                caller: live.caller.clone(),
                latency,
                tools: live.tools.clone(),
                interrupted,
            });
        }
        self.save();
    }
}

/// Loads the model and its prompt (Ollama keeps the prompt's work for the
/// next request that starts the same way), stopping at the first word.
/// `history` is just the system prompt here.
fn warm_up(cfg: &LlmConfig, history: &[Message], tools: &[ToolSpec]) -> Result<()> {
    let cancel = Arc::new(AtomicBool::new(false));
    // Only the system prompt: measured with gemma4 on Ollama, the next
    // request then reuses its work (0.5 s instead of 4 s for ~600 tokens),
    // which it did not when the warm-up also had a greeting and a question.
    let messages = history.to_vec();
    let c = cancel.clone();
    llm::stream(cfg, &messages, tools, &cancel, |_| c.store(true, Ordering::Relaxed))?;
    Ok(())
}

/// Whisper on its own thread. Finals are all transcribed, in order; of the
/// partials only the newest is, and only while no final waits.
fn start_asr(
    mut engine: WhisperEngine,
    language: Option<String>,
    events: Sender<Event>,
) -> Result<(Sender<AsrJob>, std::thread::JoinHandle<()>)> {
    let (tx, rx) = crossbeam_channel::unbounded::<AsrJob>();
    let thread = std::thread::Builder::new()
        .name("agent whisper".into())
        .spawn(move || {
            while let Ok(first) = rx.recv() {
                let mut jobs = vec![first];
                jobs.extend(rx.try_iter());
                for job in pick_jobs(jobs) {
                    let samples = &job.chunk.samples;
                    let final_ = job.chunk.kind == ChunkKind::Final;
                    // Noise, a cough, breathing: not worth a turn.
                    let noise = final_
                        && engine
                            .speech_seconds(samples)
                            .is_some_and(|s| s < crate::vad::MIN_SPEECH_SECONDS);
                    let text = if noise {
                        None
                    } else {
                        match engine.transcribe_with_prompt(samples, language.as_deref(), Some(WHISPER_PROMPT)) {
                            Ok(r) => clean_text(&r.text),
                            Err(e) => {
                                eprintln!("agent: Whisper failed: {e:#}");
                                None
                            }
                        }
                    };
                    let heard = Heard {
                        kind: job.chunk.kind,
                        text,
                        sent: job.sent,
                    };
                    if events.send(Event::Heard(heard)).is_err() {
                        return;
                    }
                }
            }
        })
        .context("could not start the Whisper thread")?;
    Ok((tx, thread))
}

/// Of the waiting jobs: every final, and the newest partial if it belongs
/// to an utterance no final has closed.
fn pick_jobs(jobs: Vec<AsrJob>) -> Vec<AsrJob> {
    let last_final = jobs
        .iter()
        .filter(|j| j.chunk.kind == ChunkKind::Final)
        .map(|j| j.chunk.utterance)
        .max();
    let mut finals = Vec::new();
    let mut partial = None;
    for j in jobs {
        match j.chunk.kind {
            ChunkKind::Final => finals.push(j),
            ChunkKind::Partial if last_final.is_none_or(|f| j.chunk.utterance > f) => partial = Some(j),
            ChunkKind::Partial => {}
        }
    }
    finals.extend(partial);
    finals
}

/// The voice on its own thread: makes each sentence's sound, in order, and
/// queues it to play, unless its turn is no longer the one to speak.
fn start_voice(
    mut synth: Box<dyn Synth>,
    output: Arc<dyn Output>,
    voice_turn: Arc<AtomicU64>,
    events: Sender<Event>,
) -> Result<Sender<(u64, usize, String)>> {
    let (tx, rx) = crossbeam_channel::unbounded::<(u64, usize, String)>();
    // Loads the voice, so the first sentence is not slower than the rest.
    let _ = synth.synthesize("Hello.", output.rate(), &|| false);
    std::thread::Builder::new()
        .name("agent tts".into())
        .spawn(move || {
            for (turn, index, text) in rx {
                let live = || voice_turn.load(Ordering::Relaxed) == turn;
                if !live() {
                    continue;
                }
                match synth.synthesize(&text, output.rate(), &|| !live()) {
                    Ok(Some(samples)) if live() => output.play(Segment { turn, index, samples }),
                    Ok(_) => {}
                    Err(e) => {
                        let _ = events.send(Event::SpeakFailed {
                            turn,
                            index,
                            error: format!("{e:#}"),
                        });
                    }
                }
            }
        })
        .context("could not start the voice thread")?;
    Ok(tx)
}

#[derive(Clone)]
struct TurnContext {
    llm: LlmConfig,
    tools: Vec<ToolSpec>,
    clinic: Arc<Mutex<Clinic>>,
}

/// One turn: the model's answer streamed into sentences, with tool calls
/// run and answered until it has something to say.
fn run_turn(turn: u64, ctx: TurnContext, mut messages: Vec<Message>, cancel: Arc<AtomicBool>, tx: Sender<Event>) {
    let send = |event: TurnEvent| {
        let _ = tx.send(Event::Turn { turn, event });
    };
    let mut first = true;
    let mut retried = false;
    // Whether anything of this turn went to the voice yet.
    let mut spoke = false;
    for round in 0..MAX_ROUNDS {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let mut text = String::new();
        let mut calls: Vec<ToolCall> = Vec::new();
        let mut splitter = if spoke {
            SentenceSplitter::continuing()
        } else {
            SentenceSplitter::new()
        };
        // Decided by the first characters: a reply that starts like JSON is
        // a tool call written out, and is not spoken.
        let mut held: Option<bool> = None;
        let result = llm::stream(&ctx.llm, &messages, &ctx.tools, &cancel, |event| {
            if first && event != StreamEvent::Done {
                first = false;
                send(TurnEvent::FirstToken(Instant::now()));
            }
            match event {
                StreamEvent::Text(t) => {
                    text.push_str(&t);
                    let piece = match held {
                        Some(true) => return,
                        Some(false) => t.as_str(),
                        None if text.trim().is_empty() => return,
                        None => {
                            let json = clinic::looks_like_json(&text);
                            held = Some(json);
                            if json {
                                return;
                            }
                            text.as_str()
                        }
                    };
                    for s in splitter.push(piece) {
                        spoke = true;
                        send(TurnEvent::Sentence(s));
                    }
                }
                StreamEvent::ToolCall(c) => calls.push(c),
                StreamEvent::Done => {}
            }
        });
        match result {
            Err(e) => return send(TurnEvent::Failed(format!("{e:#}"))),
            Ok(false) => return,
            Ok(true) => {}
        }
        if held != Some(true)
            && let Some(rest) = splitter.finish()
        {
            spoke = true;
            send(TurnEvent::Sentence(rest));
        }

        let mut json_call = false;
        if held == Some(true) && calls.is_empty() {
            match clinic::parse_text_reply(&text, &format!("json_{turn}_{round}")) {
                TextReply::Call(c) => {
                    json_call = true;
                    calls.push(c);
                }
                TextReply::Speech => {}
                TextReply::Malformed(why) if !retried => {
                    retried = true;
                    messages.push(Message::assistant(&text));
                    messages.push(Message::user(&format!(
                        "(Note from the system, not the caller: your last reply could not be used because {why}. \
                         Reply again, either with plain words for the caller or with only the JSON tool call.)"
                    )));
                    continue;
                }
                TextReply::Malformed(why) => {
                    return send(TurnEvent::Failed(format!(
                        "the model's tool call could not be read twice: {why}"
                    )));
                }
            }
        }
        if calls.is_empty() {
            return send(TurnEvent::Done);
        }
        // The tool and the model's second pass take a second or two: the
        // caller hears that something is happening, as a person would say.
        if !spoke {
            spoke = true;
            send(TurnEvent::Sentence(filler(&calls).to_string()));
        }

        let results: Vec<(ToolCall, String)> = calls
            .iter()
            .map(|c| {
                let result = ctx.clinic.lock().map(|mut cl| cl.call(c)).unwrap_or_default();
                (c.clone(), result)
            })
            .collect();
        let mut history = Vec::new();
        if json_call {
            history.push(Message::assistant(&text));
            for (c, r) in &results {
                history.push(Message::user(&format!("TOOL RESULT {}: {r}", c.name)));
            }
        } else {
            // What was said before the call is spoken and goes into the
            // history with the rest of the answer.
            history.push(Message::assistant_calls("", calls.clone()));
            for (c, r) in &results {
                history.push(Message::tool_result(c, r));
            }
        }
        // This turn's request carries what was said before the call too.
        if !json_call {
            messages.push(Message::assistant_calls(&text, calls.clone()));
            messages.extend(history.iter().skip(1).cloned());
        } else {
            messages.extend(history.iter().cloned());
        }
        send(TurnEvent::Tools {
            history,
            calls: results,
        });
    }
    send(TurnEvent::Failed("too many tool calls in one turn".into()));
}

/// Said while a tool runs, when the model said nothing before calling it.
fn filler(calls: &[ToolCall]) -> &'static str {
    if calls.iter().any(|c| c.name == "book_appointment") {
        "One moment while I book that."
    } else {
        "Let me check."
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(utterance: u64, kind: ChunkKind) -> AsrJob {
        AsrJob {
            chunk: Chunk {
                utterance,
                kind,
                start: 0.0,
                samples: Vec::new(),
            },
            sent: Instant::now(),
        }
    }

    #[test]
    fn whisper_skips_stale_partials_but_never_a_final() {
        let picked = pick_jobs(vec![
            job(1, ChunkKind::Partial),
            job(1, ChunkKind::Partial),
            job(1, ChunkKind::Final),
            job(2, ChunkKind::Partial),
            job(2, ChunkKind::Partial),
        ]);
        let got: Vec<(u64, ChunkKind)> = picked.iter().map(|j| (j.chunk.utterance, j.chunk.kind)).collect();
        assert_eq!(got, [(1, ChunkKind::Final), (2, ChunkKind::Partial)]);
        let only_partials = pick_jobs(vec![job(4, ChunkKind::Partial), job(4, ChunkKind::Partial)]);
        assert_eq!(only_partials.len(), 1);
    }

    #[test]
    fn options_are_checked() {
        assert!(parse_args(&["--api".into(), "gpt".into()]).is_err());
        assert!(parse_args(&["--tools".into(), "xml".into()]).is_err());
        assert!(parse_args(&["--pause".into(), "9".into()]).is_err());
        assert!(parse_args(&["--bogus".into()]).is_err());
    }
}
