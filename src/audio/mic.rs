//! Microphone capture with cpal (Core Audio on macOS).

use anyhow::{Context as _, Result, anyhow, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use crossbeam_channel::Sender;

use super::{AudioPacket, Capture, Converter};
use crate::transcript::Speaker;

/// Stops the capture thread when dropped.
struct MicGuard {
    stop: Option<Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for MicGuard {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Starts capturing the default input device as `Speaker::Me`.
pub fn start_mic(tx: Sender<AudioPacket>) -> Result<Capture> {
    // A cpal stream must stay on the thread that created it on macOS,
    // so it lives on its own thread until the guard is dropped.
    let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<String>>(1);
    let (stop_tx, stop_rx) = crossbeam_channel::bounded::<()>(0);
    let thread = std::thread::Builder::new()
        .name("mic".into())
        .spawn(move || match open_stream(tx) {
            Ok((stream, name)) => {
                let _ = ready_tx.send(Ok(name));
                // Blocks until the guard drops the sender.
                let _ = stop_rx.recv();
                drop(stream);
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e));
            }
        })
        .context("could not start microphone thread")?;

    let guard = MicGuard {
        stop: Some(stop_tx),
        thread: Some(thread),
    };
    let name = ready_rx
        .recv()
        .map_err(|_| anyhow!("microphone thread ended unexpectedly"))??;
    Ok(Capture::new(name, guard))
}

fn open_stream(tx: Sender<AudioPacket>) -> Result<(cpal::Stream, String)> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .context("no microphone found. Check System Settings > Sound > Input")?;
    let name = device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "Microphone".into());
    let config = device
        .default_input_config()
        .context("could not read the microphone's audio format")?;
    let converter = Converter::new(Speaker::Me, config.sample_rate(), config.channels() as usize, tx);
    let stream = match config.sample_format() {
        SampleFormat::F32 => build::<f32>(&device, config.config(), converter)?,
        SampleFormat::I16 => build::<i16>(&device, config.config(), converter)?,
        SampleFormat::I32 => build::<i32>(&device, config.config(), converter)?,
        SampleFormat::U16 => build::<u16>(&device, config.config(), converter)?,
        other => bail!("unsupported microphone sample format {other}"),
    };
    stream.play().context(
        "could not start the microphone. Allow microphone access in System Settings > Privacy & Security > Microphone",
    )?;
    Ok((stream, name))
}

fn build<T>(device: &cpal::Device, config: cpal::StreamConfig, mut converter: Converter) -> Result<cpal::Stream>
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
                converter.push_interleaved(&scratch);
            },
            |err| eprintln!("microphone error: {err}"),
            None,
        )
        .context("could not open the microphone")
}
