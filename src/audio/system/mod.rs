//! System audio capture: everything the computer plays, i.e. the other
//! people in the meeting. macOS records it with ScreenCaptureKit, Windows
//! with WASAPI loopback on the default output (in `audio::mic`, since it is
//! a cpal stream), and Linux from the sound server's monitor of the default
//! output (PulseAudio, or PipeWire through pipewire-pulse).

use anyhow::Result;
use crossbeam_channel::Sender;

use super::{AudioPacket, Capture};

#[cfg(not(any(target_os = "macos", windows)))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub fn start_system_audio(tx: Sender<AudioPacket>) -> Result<Capture> {
    macos::start(tx)
}

#[cfg(windows)]
pub fn start_system_audio(tx: Sender<AudioPacket>) -> Result<Capture> {
    super::mic::start(super::mic::Input::Loopback, tx)
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn start_system_audio(tx: Sender<AudioPacket>) -> Result<Capture> {
    linux::start(tx)
}

/// parec's arguments: raw mono f32 samples at `rate` from the monitor of
/// whatever output is the default right now, so switching speakers between
/// meetings needs no setting.
#[cfg(any(not(any(target_os = "macos", windows)), test))]
fn parec_args(rate: u32) -> Vec<String> {
    [
        "--device=@DEFAULT_MONITOR@",
        "--raw",
        "--format=float32le",
        "--channels=1",
        "--latency-msec=50",
        "--client-name=Meeting Transcriber",
        "--stream-name=Others",
    ]
    .iter()
    .map(|a| a.to_string())
    .chain([format!("--rate={rate}")])
    .collect()
}

/// Turns a byte stream of little-endian f32 samples into samples. A read
/// can end inside a sample; those bytes wait in `carry` for the next one.
#[cfg(any(not(any(target_os = "macos", windows)), test))]
fn decode_f32le(carry: &mut Vec<u8>, bytes: &[u8]) -> Vec<f32> {
    carry.extend_from_slice(bytes);
    let (whole, _) = carry.as_chunks::<4>();
    let samples: Vec<f32> = whole.iter().map(|b| f32::from_le_bytes(*b)).collect();
    carry.drain(..samples.len() * 4);
    samples
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parec_records_the_default_monitor_as_raw_mono_floats() {
        let args = parec_args(16_000);
        for want in [
            "--device=@DEFAULT_MONITOR@",
            "--raw",
            "--format=float32le",
            "--channels=1",
            "--rate=16000",
        ] {
            assert!(args.iter().any(|a| a == want), "missing {want} in {args:?}");
        }
        // One argument each: parec sees "Meeting Transcriber" whole.
        assert!(args.contains(&"--client-name=Meeting Transcriber".to_string()));
    }

    #[test]
    fn samples_split_across_reads_are_joined() {
        let bytes: Vec<u8> = [0.5f32, -1.0, 0.25].iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut carry = Vec::new();
        assert_eq!(decode_f32le(&mut carry, &bytes[..6]), vec![0.5]);
        assert_eq!(carry.len(), 2);
        assert_eq!(decode_f32le(&mut carry, &bytes[6..7]), Vec::<f32>::new());
        assert_eq!(decode_f32le(&mut carry, &bytes[7..]), vec![-1.0, 0.25]);
        assert!(carry.is_empty());
    }
}
