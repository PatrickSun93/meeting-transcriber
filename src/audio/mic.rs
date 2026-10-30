//! Capture with cpal: the microphone on every system (Core Audio, WASAPI,
//! ALSA), and on Windows also the speakers, recorded in WASAPI loopback mode.

use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result, anyhow, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use crossbeam_channel::{RecvTimeoutError, Sender};

use super::silence::{Paced, TICK};
use super::{AudioPacket, Capture, Converter};
use crate::platform;
use crate::transcript::Speaker;

/// What a cpal capture records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Input {
    /// The default input device, as Me.
    Microphone,
    /// The default output device in loopback mode, as Others. WASAPI only:
    /// cpal turns an input stream on an output device into loopback.
    #[cfg(windows)]
    Loopback,
}

impl Input {
    fn speaker(self) -> Speaker {
        match self {
            Input::Microphone => Speaker::Me,
            #[cfg(windows)]
            Input::Loopback => Speaker::Others,
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Input::Microphone => "microphone",
            #[cfg(windows)]
            Input::Loopback => "PC audio",
        }
    }
}

/// Stops the capture thread when dropped.
struct CaptureGuard {
    stop: Option<Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for CaptureGuard {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Starts capturing the default input device as `Speaker::Me`.
pub fn start_mic(tx: Sender<AudioPacket>) -> Result<Capture> {
    start(Input::Microphone, tx)
}

pub(super) fn start(input: Input, tx: Sender<AudioPacket>) -> Result<Capture> {
    // A cpal stream must stay on the thread that created it on macOS,
    // so it lives on its own thread until the guard is dropped.
    let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<String>>(1);
    let (stop_tx, stop_rx) = crossbeam_channel::bounded::<()>(0);
    let thread = std::thread::Builder::new()
        .name(input.noun().into())
        .spawn(move || match open_stream(input, tx) {
            Ok((stream, name, paced)) => {
                let _ = ready_tx.send(Ok(name));
                // Ticks until the guard drops the sender.
                while let Err(RecvTimeoutError::Timeout) = stop_rx.recv_timeout(TICK) {
                    paced.lock().unwrap_or_else(|e| e.into_inner()).tick();
                }
                drop(stream);
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e));
            }
        })
        .with_context(|| format!("could not start the {} thread", input.noun()))?;

    let guard = CaptureGuard {
        stop: Some(stop_tx),
        thread: Some(thread),
    };
    let name = ready_rx
        .recv()
        .map_err(|_| anyhow!("{} thread ended unexpectedly", input.noun()))??;
    Ok(Capture::new(name, guard))
}

type SharedFeed = Arc<Mutex<Paced>>;

fn open_stream(input: Input, tx: Sender<AudioPacket>) -> Result<(cpal::Stream, String, SharedFeed)> {
    let host = cpal::default_host();
    let (device, config) = match input {
        Input::Microphone => {
            let device = host
                .default_input_device()
                .with_context(|| format!("no microphone found. {}", platform::NO_MIC_HINT))?;
            let config = device
                .default_input_config()
                .context("could not read the microphone's audio format")?;
            (device, config)
        }
        #[cfg(windows)]
        Input::Loopback => {
            let device = host
                .default_output_device()
                .context("no speakers or headphones found. Check Settings > System > Sound > Output")?;
            let config = device
                .default_output_config()
                .context("could not read the speakers' audio format")?;
            (device, config)
        }
    };
    let name = device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| input.noun().to_string());
    let converter = Converter::new(input.speaker(), config.sample_rate(), config.channels() as usize, tx);
    let paced = match input {
        Input::Microphone => Paced::new(converter),
        // Loopback sends nothing while nothing plays.
        #[cfg(windows)]
        Input::Loopback => Paced::with_silence_fill(converter, config.sample_rate()),
    };
    let paced = Arc::new(Mutex::new(paced));
    let stream = match config.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, config.config(), paced.clone(), input)?,
        SampleFormat::I16 => build::<i16>(&device, config.config(), paced.clone(), input)?,
        SampleFormat::I32 => build::<i32>(&device, config.config(), paced.clone(), input)?,
        SampleFormat::U16 => build::<u16>(&device, config.config(), paced.clone(), input)?,
        other => bail!("unsupported {} sample format {other}", input.noun()),
    };
    stream.play().with_context(|| match input {
        Input::Microphone => format!("could not start the microphone. {}", platform::MIC_ACCESS_HINT),
        #[cfg(windows)]
        Input::Loopback => "could not start recording PC audio".to_string(),
    })?;
    let name = match input {
        Input::Microphone => name,
        #[cfg(windows)]
        Input::Loopback => format!("{} ({name})", platform::SYSTEM_AUDIO),
    };
    Ok((stream, name, paced))
}

fn build<T>(device: &cpal::Device, config: cpal::StreamConfig, paced: SharedFeed, input: Input) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let mut scratch = Vec::new();
    device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                scratch.clear();
                scratch.extend(data.iter().map(|s| f32::from_sample(*s)));
                paced
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push_interleaved(&scratch);
            },
            move |err| eprintln!("{} error: {err}", input.noun()),
            None,
        )
        .with_context(|| format!("could not open the {}", input.noun()))
}
