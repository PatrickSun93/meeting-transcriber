//! Screen recording on macOS: ScreenCaptureKit records the area straight
//! into the MP4 file (`SCRecordingOutput`, macOS 15 and later), H.264 from
//! the Mac's own encoder, with the pointer, and with the computer's sound
//! and the microphone, each on an audio track of its own. This app's
//! windows (the recording's controls) are left out, as in screenshots.
//! Frames carry the time they were taken, so a busy Mac drops some rather
//! than speeding the video up.

use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use core_graphics::geometry::CGPoint;
use screencapturekit::prelude::*;
use screencapturekit::recording_output::{
    RecordingCallbacks, SCRecordingOutput, SCRecordingOutputCodec, SCRecordingOutputConfiguration,
    SCRecordingOutputFileType,
};

use super::{FPS, Recorded, Sound, video_size};
use crate::capture::macos::{PERMISSION_HINT, check_allowed, display_at, kit_filter};
use crate::capture::scrolling::Area;

/// For the recording to start once the stream has.
const START_WAIT: Duration = Duration::from_secs(5);
/// The sound's rate and channels, as the Mac's own recordings have it.
const SAMPLE_RATE: i32 = 48_000;
const CHANNELS: i32 = 2;

pub fn unsupported() -> Option<&'static str> {
    (!SCRecordingOutput::is_available()).then_some("Screen recording needs macOS 15 or later.")
}

/// What the recording has said so far, from ScreenCaptureKit's threads.
#[derive(Default)]
struct Status {
    started: bool,
    failed: Option<String>,
}

type Shared = Arc<(Mutex<Status>, Condvar)>;

pub struct Recorder {
    stream: SCStream,
    output: SCRecordingOutput,
    status: Shared,
}

impl Recorder {
    pub fn start(area: &Area, path: &Path, sound: Sound) -> Result<Recorder> {
        check_allowed()?;
        let scale = f64::from(area.display.scale.max(0.25));
        let [x, y] = area.center();
        let id = display_at(CGPoint::new(f64::from(x) / scale, f64::from(y) / scale))?
            .context("the display of the area is no longer there")?;
        let r = area.rect;
        let points = |v: u32| f64::from(v) / scale;
        let [width, height] = video_size(r.width, r.height);
        let mut config = SCStreamConfiguration::new()
            .with_source_rect(CGRect::new(points(r.x), points(r.y), points(r.width), points(r.height)))
            .with_width(width)
            .with_height(height)
            .with_shows_cursor(true)
            .with_minimum_frame_interval(&CMTime::new(1, FPS as i32))
            .with_captures_audio(sound.computer)
            .with_excludes_current_process_audio(true)
            .with_sample_rate(SAMPLE_RATE)
            .with_channel_count(CHANNELS);
        if sound.microphone {
            config = config
                .with_captures_microphone(true)
                .map_err(|e| anyhow!("could not record the microphone: {e}"))?;
        }
        let status = Shared::default();
        let output_config = SCRecordingOutputConfiguration::new()
            .map_err(|e| anyhow!("could not set up the recording: {e}"))?
            .with_output_url(path)
            .map_err(|e| anyhow!("could not record into {}: {e}", path.display()))?
            .with_video_codec(SCRecordingOutputCodec::H264)
            .with_output_file_type(SCRecordingOutputFileType::MP4);
        let output = SCRecordingOutput::new_with_delegate(&output_config, callbacks(&status))
            .context("could not set up the recording")?;
        let stream = SCStream::new(&kit_filter(id)?, &config).map_err(|e| anyhow!("{PERMISSION_HINT} ({e})"))?;
        stream
            .add_recording_output(&output)
            .map_err(|e| anyhow!("could not set up the recording: {e}"))?;
        stream
            .start_capture()
            .map_err(|e| anyhow!("the recording did not start: {e}. {PERMISSION_HINT}"))?;
        let recorder = Recorder { stream, output, status };
        let (lock, changed) = &*recorder.status;
        let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let (guard, _) = changed
            .wait_timeout_while(guard, START_WAIT, |s| !s.started && s.failed.is_none())
            .unwrap_or_else(|e| e.into_inner());
        let failed = guard.failed.clone();
        let started = guard.started;
        drop(guard);
        if let Some(why) = failed {
            let _ = recorder.stream.stop_capture();
            bail!("the recording did not start: {why}");
        }
        if !started {
            let _ = recorder.stream.stop_capture();
            bail!("the recording did not start in time");
        }
        Ok(recorder)
    }

    pub fn failure(&self) -> Option<String> {
        self.status.0.lock().unwrap_or_else(|e| e.into_inner()).failed.clone()
    }

    pub fn stop(self) -> Result<Recorded> {
        // Stops the stream too, and waits for the file to be finished.
        let removed = self.stream.remove_recording_output(&self.output);
        let _ = self.stream.stop_capture();
        if let Some(why) = self.failure() {
            bail!("the recording failed: {why}");
        }
        removed.map_err(|e| anyhow!("the recording could not be finished: {e}"))?;
        let seconds = self.output.recorded_duration().as_seconds().unwrap_or(0.0);
        Ok(Recorded { seconds })
    }
}

/// The recording's news, into `status`.
fn callbacks(status: &Shared) -> RecordingCallbacks {
    let (started, failed) = (status.clone(), status.clone());
    RecordingCallbacks::new()
        .on_start(move || update(&started, |s| s.started = true))
        .on_fail(move |why| update(&failed, |s| s.failed = Some(why)))
}

fn update(shared: &Shared, change: impl FnOnce(&mut Status)) {
    let (lock, changed) = &**shared;
    change(&mut lock.lock().unwrap_or_else(|e| e.into_inner()));
    changed.notify_all();
}
