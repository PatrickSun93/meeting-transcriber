//! Linux: the sound server's monitor of the default output, recorded with
//! `parec` (PulseAudio's recorder, which also talks to PipeWire through
//! pipewire-pulse). A child process keeps this free of extra libraries:
//! ALSA alone cannot see what other apps play.

use std::io::Read as _;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, anyhow, bail};
use crossbeam_channel::{RecvTimeoutError, Sender};

use super::{decode_f32le, parec_args};
use crate::audio::dsp::SAMPLE_RATE;
use crate::audio::silence::{Paced, TICK};
use crate::audio::{AudioPacket, Capture, Converter};
use crate::platform;
use crate::transcript::Speaker;

const INSTALL_HINT: &str = "recording computer audio needs parec, from PulseAudio's tools \
    (package pulseaudio-utils on Debian, Ubuntu and Fedora; libpulse on Arch). It works with PipeWire too";

/// parec gives up at once when there is no sound server; this long without
/// exiting means it is recording.
const STARTUP_CHECK: Duration = Duration::from_millis(300);

/// Stops parec and the threads that read it.
struct ParecGuard {
    child: Child,
    stop: Option<Sender<()>>,
    threads: Vec<JoinHandle<()>>,
}

impl Drop for ParecGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        drop(self.stop.take());
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

pub fn start(tx: Sender<AudioPacket>) -> Result<Capture> {
    let mut child = Command::new("parec")
        .args(parec_args(SAMPLE_RATE))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => anyhow!("{INSTALL_HINT}"),
            _ => anyhow!(e).context("could not start parec"),
        })?;

    // Drained on its own thread, so a chatty parec never blocks on a full pipe.
    let mut stderr = child.stderr.take().context("parec has no error output")?;
    let errors = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });
    let deadline = Instant::now() + STARTUP_CHECK;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().context("could not check on parec")? {
            let text = errors.join().unwrap_or_default();
            let reason = text.trim();
            bail!(
                "could not record {} (parec {status}{}{reason}). Is PulseAudio or PipeWire running?",
                platform::SYSTEM_AUDIO,
                if reason.is_empty() { "" } else { ": " }
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    let mut stdout = child.stdout.take().context("parec has no output")?;
    let converter = Converter::new(Speaker::Others, SAMPLE_RATE, 1, tx);
    // A monitor normally sends silence too, but a suspended output may not.
    let paced = Arc::new(Mutex::new(Paced::with_silence_fill(converter, SAMPLE_RATE)));
    let reader = {
        let paced = paced.clone();
        std::thread::Builder::new()
            .name("parec".into())
            .spawn(move || {
                let mut buf = vec![0u8; 16 * 1024];
                let mut carry = Vec::new();
                // Ends when parec is killed (end of file).
                while let Ok(n @ 1..) = stdout.read(&mut buf) {
                    let samples = decode_f32le(&mut carry, &buf[..n]);
                    paced
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push_interleaved(&samples);
                }
            })
            .context("could not start the parec reader")?
    };
    let (stop_tx, stop_rx) = crossbeam_channel::bounded::<()>(0);
    let ticker = std::thread::Builder::new()
        .name("parec-silence".into())
        .spawn(move || {
            while let Err(RecvTimeoutError::Timeout) = stop_rx.recv_timeout(TICK) {
                paced.lock().unwrap_or_else(|e| e.into_inner()).tick();
            }
        })
        .context("could not start the parec timer")?;

    let guard = ParecGuard {
        child,
        stop: Some(stop_tx),
        threads: vec![reader, ticker],
    };
    // Joining `errors` would wait for parec to exit; it ends with parec.
    drop(errors);
    Ok(Capture::new("Computer audio (default output)".into(), guard))
}
