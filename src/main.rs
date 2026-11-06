// Release builds on Windows are GUI programs, so opening the app does not
// also open a console window. The CLI reattaches to its caller's console.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use anyhow::{Context as _, Result, bail};
use meeting_transcriber::media::{self, Tracks};
use meeting_transcriber::models::{self, DEFAULT_MODEL, ModelInfo};
use meeting_transcriber::settings::Settings;
use meeting_transcriber::transcriber::{SpeechToText as _, WhisperEngine};
use meeting_transcriber::transcript::format_time;
use meeting_transcriber::{app, cli, import, wav};

const USAGE: &str = "\
Meeting Transcriber

  meeting-transcriber [--hidden]
      Opens the app window. --hidden (used at login) starts in the menu
      bar or tray with the window closed.

  meeting-transcriber dictate start|stop|toggle      (Linux)
      Starts or stops dictation in the running app, as holding and
      letting go of the hotkey does. Bind `dictate toggle` to a shortcut
      in your desktop's keyboard settings (on Wayland, or for any key).

  meeting-transcriber install [--no-desktop]          (Windows)
      Copies the program (and its graphics-card builds) to
      %LOCALAPPDATA%\\Programs\\Meeting Transcriber and adds shortcuts in
      the Start menu and on the desktop. uninstall takes them away.

  meeting-transcriber doctor [--json]
      Reports what the app sees here: version, settings file, folders,
      models, the hotkey, whether the app is running, permissions, and
      anything that looks wrong. For bug reports and AI agents.

  meeting-transcriber log [--lines N]
      Prints the end of the app's log (errors from recording, Whisper
      and dictation), kept in the app data folder.

  meeting-transcriber gpu [--json]
  meeting-transcriber gpu install-cuda [--from URL_OR_ZIP]
      Says where Whisper runs (processor, Vulkan, CUDA) and why. With an
      NVIDIA card, install-cuda downloads CUDA support (about 600 MB, from
      the release of this version) or unpacks a downloaded add-on zip.

  meeting-transcriber keys [--once] [--save] [--json]
      Prints every key or combination you hold and let go of, as the
      dictation hotkey watcher sees it, and its setting name. --once
      stops after one; --save makes it the dictation hotkey.

  meeting-transcriber settings [--json]
  meeting-transcriber settings path | get KEY | set KEY VALUE
      Lists, reads or changes settings (checked before saving), e.g.
      `settings set dictation_hotkey ctrl+shift+vk:68`.

  meeting-transcriber transcribe [--me FILE] [--others FILE]
                                 [--model PATH_OR_ID] [--language CODE]
      Transcribes recordings with the same pipeline, printing each line.
      --me / --others take the microphone and meeting-audio tracks.
      --model defaults to the downloaded large-v3-turbo-q5_0 model.
      --language defaults to auto-detect (e.g. en, zh).

  meeting-transcriber transcribe --file FILE [--no-speakers] [--save]
                                 [--model PATH_OR_ID] [--language CODE]
      Transcribes one recording (a Teams or Zoom video: MP4, MOV, M4A,
      MP3, WAV, FLAC, MKV), its voices told apart as Speaker 1, 2, …
      (the speaker models, about 33 MB, are downloaded the first time).
      --no-speakers keeps one track. --save adds it to the meetings.
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
    // The processor build hands over to a graphics-card build beside it
    // when there is a card for it (Windows; see `gpu`).
    if let Some(code) = meeting_transcriber::gpu::hand_off(&args) {
        std::process::exit(code);
    }
    match args.first().map(String::as_str) {
        None => run_app(false),
        Some("--hidden") => run_app(true),
        Some("transcribe") => run_transcribe(&args[1..]),
        Some("dictate") => run_dictate(&args[1..]),
        Some("install") => {
            let (set, _) = flags(&args[1..], &["--no-desktop"])?;
            let exe = meeting_transcriber::platform::install(!set.contains(&"--no-desktop"))?;
            println!("Installed {}", exe.display());
            Ok(())
        }
        Some("uninstall") => {
            let dir = meeting_transcriber::platform::uninstall()?;
            println!(
                "Removed the shortcuts and the login entry. Delete {} to remove the program; \
                 your meetings and settings stay.",
                dir.display()
            );
            Ok(())
        }
        Some("doctor") => {
            let (_, json) = flags(&args[1..], &[])?;
            let report = cli::doctor();
            if json {
                println!("{}", report.to_json());
            } else {
                print!("{}", report.to_text());
            }
            Ok(())
        }
        Some("log") => {
            let lines = match &args[1..] {
                [] => 200,
                [flag, n] if flag == "--lines" => n.parse().context("--lines takes a number")?,
                _ => bail!("log takes only --lines N\n\n{USAGE}"),
            };
            match meeting_transcriber::logfile::tail(lines) {
                Some(text) => println!("{text}"),
                None => eprintln!(
                    "No log yet at {}: the app writes one when opened.",
                    meeting_transcriber::logfile::log_path().display()
                ),
            }
            Ok(())
        }
        Some("gpu") => {
            let json = args.iter().any(|a| a == "--json");
            let rest: Vec<String> = args[1..].iter().filter(|a| *a != "--json").cloned().collect();
            cli::run_gpu(&rest, json)
        }
        Some("keys") => {
            let (set, json) = flags(&args[1..], &["--once", "--save"])?;
            cli::run_keys(set.contains(&"--once"), set.contains(&"--save"), json)
        }
        Some("settings") => {
            let json = args.iter().any(|a| a == "--json");
            let rest: Vec<String> = args[1..].iter().filter(|a| *a != "--json").cloned().collect();
            cli::run_settings(&rest, json)
        }
        Some("-h" | "--help" | "help") => {
            print!("{USAGE}");
            Ok(())
        }
        Some(other) => bail!("unknown command {other:?}\n\n{USAGE}"),
    }
}

/// Reads a command's flags: which of `allowed` were given, and `--json`.
fn flags<'a>(args: &[String], allowed: &[&'a str]) -> Result<(Vec<&'a str>, bool)> {
    let mut given = Vec::new();
    let mut json = false;
    for arg in args {
        if arg == "--json" {
            json = true;
        } else if let Some(flag) = allowed.iter().find(|f| **f == arg) {
            given.push(*flag);
        } else {
            bail!("unknown option {arg:?}\n\n{USAGE}");
        }
    }
    Ok((given, json))
}

fn run_app(hidden: bool) -> Result<()> {
    meeting_transcriber::logfile::start();
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

/// `transcribe --file`: one recording, its voices told apart.
fn transcribe_file(
    file: &Path,
    tracks: Tracks,
    model_path: &Path,
    language: Option<&str>,
    speakers: bool,
    save_to: Option<&Settings>,
) -> Result<()> {
    let loading = Instant::now();
    let mut engine = WhisperEngine::load(model_path)?;
    eprintln!(
        "Loaded {} in {:.1}s",
        model_path.display(),
        loading.elapsed().as_secs_f32()
    );
    let started = Instant::now();
    let mut last = None;
    let (lines, duration) = import::transcribe_file(
        file,
        tracks,
        speakers,
        Arc::new(AtomicBool::new(false)),
        |stage| {
            // Each step once, not every packet.
            let step = std::mem::discriminant(&stage);
            if last.replace(step) != Some(step) {
                eprintln!("{}", import::stage_text(&stage));
            }
        },
        |samples| engine.transcribe(samples, language),
    )?;
    for line in &lines {
        println!("[{}] {}: {}", format_time(line.start), line.label(), line.text);
    }
    let took = started.elapsed().as_secs_f32();
    eprintln!(
        "{} lines, {:.1}s of audio in {:.1}s ({:.2}x real time)",
        lines.len(),
        duration,
        took,
        took / duration.max(0.001)
    );
    if let Some(settings) = save_to {
        let path = import::save_new(
            &settings.meetings_dir_path(),
            &import::meeting_for(file, lines, duration),
        )?;
        eprintln!("Saved {}", path.display());
    }
    Ok(())
}

fn run_transcribe(args: &[String]) -> Result<()> {
    let mut me = None;
    let mut others = None;
    let mut file = None;
    let mut speakers = true;
    let mut save = false;
    let mut model = None;
    let mut language = None;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let mut value = || it.next().with_context(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--me" => me = Some(PathBuf::from(value()?)),
            "--others" => others = Some(PathBuf::from(value()?)),
            "--file" => file = Some(PathBuf::from(value()?)),
            "--no-speakers" => speakers = false,
            "--save" => save = true,
            "--model" => model = Some(value()?.clone()),
            "--language" => language = Some(value()?.clone()),
            other => bail!("unknown option {other:?}\n\n{USAGE}"),
        }
    }
    if file.is_some() && (me.is_some() || others.is_some()) {
        bail!("--file is used on its own, without --me or --others\n\n{USAGE}");
    }
    if file.is_none() && me.is_none() && others.is_none() {
        bail!("give --file, or at least one of --me or --others\n\n{USAGE}");
    }
    if file.is_none() && (!speakers || save) {
        bail!("--no-speakers and --save go with --file\n\n{USAGE}");
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

    if let Some(file) = file {
        let tracks = Tracks::for_file(&file, &settings.meetings_dir_path());
        return transcribe_file(
            &file,
            tracks,
            &model_path,
            language.as_deref(),
            speakers,
            save.then_some(&settings),
        );
    }

    let read = |p: PathBuf| media::decode(&p, Tracks::First, &AtomicBool::new(false), |_| {});
    let me = me.map(read).transpose()?;
    let others = others.map(read).transpose()?;
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
                line.label(),
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
