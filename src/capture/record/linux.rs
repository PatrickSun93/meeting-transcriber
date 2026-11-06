//! Screen recording on Linux: not there yet, so nothing is ever recorded.

use std::path::Path;

use anyhow::{Result, bail};

use super::{Recorded, Sound};
use crate::capture::scrolling::Area;

const UNSUPPORTED: &str = "Screen recording is not supported on Linux yet.";

pub fn unsupported() -> Option<&'static str> {
    Some(UNSUPPORTED)
}

/// No recording can be made, so there is none of these.
pub enum Recorder {}

impl Recorder {
    pub fn start(_area: &Area, _path: &Path, _sound: Sound) -> Result<Recorder> {
        bail!("{UNSUPPORTED}")
    }

    pub fn failure(&self) -> Option<String> {
        match *self {}
    }

    pub fn stop(self) -> Result<Recorded> {
        match self {}
    }
}
