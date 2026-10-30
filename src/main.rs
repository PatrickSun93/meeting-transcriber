// Release builds on Windows are GUI programs, so opening the app does not
// also open a console window. The CLI reattaches to its caller's console.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context as _, Result, bail};
use meeting_transcriber::models::{self, DEFAULT_MODEL, ModelInfo};
use meeting_transcriber::settings::Settings;
use meeting_transcriber::transcriber::WhisperEngine;
use meeting_transcriber::transcript::format_time;
use meeting_transcriber::{app, wav};

const USAGE: &str = "\
Meeting Transcriber

  meeting-transcriber [--hidden]
      Opens the app window. --hidden (used at login) starts in the menu
      bar or tray with the window closed.

  meeting-transcriber dictate start|stop|toggle      (Linux)
      Starts or stops dictation in the running app, as holding and
      letting go of the hotkey does. Bind `dictate toggle` to a shortcut
      in your desktop's keyboard settings (on Wayland, or for any key).

  meeting-transcriber transcribe [--me FILE.wav] [--others FILE.wav]
                                 [--model PATH_OR_ID] [--language CODE]
      Transcribes recordings with the same pipeline, printing each line.
      --me / --others take the microphone and meeting-audio tracks.
      --model defaults to the downloaded large-v3-turbo-q5_0 model.
      --language defaults to auto-detect (e.g. en, zh).
";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Only the command line: a window attached to the terminal it was
    // started from would be killed, unsaved notes and all, when that
    // terminal is closed or gets Ctrl+C.
    #[cfg(windows)]
    if !args.is_empty() {
        meeting_transcriber::platform::attach_console();
    }
    match args.first().map(String::as_str) {
        None => run_app(false),
        Some("--hidden") => run_app(true),
        Some("transcribe") => run_transcribe(&args[1..]),
        Some("dictate") => run_dictate(&args[1..]),
        Some("-h" | "--help" | "help") => {
            print!("{USAGE}");
            Ok(())
        }
        Some(other) => bail!("unknown command {other:?}\n\n{USAGE}"),
    }
}

fn run_app(hidden: bool) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Meeting Transcriber")
            // Matches the .desktop file on Linux (Wayland), for the icon and grouping.
            .with_app_id("meeting-transcriber")
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([720.0, 420.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Meeting Transcriber",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, hidden)))),
    )
    .map_err(|e| anyhow::anyhow!("could not open the window: {e}"))
}

/// Tells the running app to start or stop dictating.
#[cfg(not(any(target_os = "macos", windows)))]
fn run_dictate(args: &[String]) -> Result<()> {
    let [command] = args else {
        bail!("dictate takes one of start, stop or toggle\n\n{USAGE}");
    };
    let answer = meeting_transcriber::platform::send_dictate_command(command)?;
    eprintln!("Dictation {answer}");
    Ok(())
}

/// The hotkey itself works everywhere else; the command is for Linux.
#[cfg(any(target_os = "macos", windows))]
fn run_dictate(_args: &[String]) -> Result<()> {
    bail!("`dictate` is for Linux; here, hold the dictation hotkey instead")
}

fn run_transcribe(args: &[String]) -> Result<()> {
    let mut me = None;
    let mut others = None;
    let mut model = None;
    let mut language = None;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let mut value = || it.next().with_context(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--me" => me = Some(PathBuf::from(value()?)),
            "--others" => others = Some(PathBuf::from(value()?)),
            "--model" => model = Some(value()?.clone()),
            "--language" => language = Some(value()?.clone()),
            other => bail!("unknown option {other:?}\n\n{USAGE}"),
        }
    }
    if me.is_none() && others.is_none() {
        bail!("give at least one of --me or --others\n\n{USAGE}");
    }

    // Same models folder as the app, including models an unfinished move
    // has not brought over yet.
    let settings = Settings::load();
    models::set_configured_dir(settings.models_dir_path());
    models::set_move_source(settings.models_move_from_path());
    let model_path = match model {
        Some(m) => ModelInfo::by_id(&m).map_or_else(|| PathBuf::from(&m), |info| info.path()),
        None => DEFAULT_MODEL.path(),
    };
    if !model_path.is_file() {
        bail!(
            "model not found at {}. Download it from the app, or pass --model /path/to/ggml-model.bin",
            model_path.display()
        );
    }

    let me = me.map(|p| wav::read_wav(&p)).transpose()?;
    let others = others.map(|p| wav::read_wav(&p)).transpose()?;
    let audio_seconds = me.iter().chain(&others).map(|t| t.len()).max().unwrap_or(0) as f32 / 16_000.0;

    let loading = Instant::now();
    let mut engine = WhisperEngine::load(&model_path)?;
    eprintln!(
        "Loaded {} in {:.1}s",
        model_path.display(),
        loading.elapsed().as_secs_f32()
    );

    let started = Instant::now();
    let transcript = wav::transcribe_tracks(
        &mut engine,
        me.as_deref(),
        others.as_deref(),
        language.as_deref(),
        |line| {
            eprintln!(
                "[{}] {}: {}{}",
                format_time(line.start),
                line.speaker.label(),
                line.text,
                line.language.as_deref().map(|l| format!("  ({l})")).unwrap_or_default()
            );
        },
    )?;
    let took = started.elapsed().as_secs_f32();
    // Lines above were streamed as they finished; this is the final
    // transcript after echo removal.
    print!("{}", transcript.to_text());
    eprintln!(
        "{} lines, {:.1}s of audio in {:.1}s ({:.2}x real time), {} echo lines removed",
        transcript.lines().len(),
        audio_seconds,
        took,
        took / audio_seconds.max(0.001),
        transcript.echo_removed
    );
    Ok(())
}
