//! Text to speech. On the Mac this is the system's `say`, one process per
//! sentence, writing a WAV file that is then played by `player`. Playing it
//! ourselves (rather than letting `say` play it) tells us exactly when the
//! sound starts and how much of it was heard before a barge-in, lets the
//! next sentence be made while this one plays, and gives the samples a
//! phone line would need. The process is killed if the turn is cancelled.
//!
//! Other systems have no voice yet: `Say::new` says so.

use anyhow::Result;

/// Makes speech for one sentence.
pub trait Synth: Send {
    /// Mono samples at `rate`, or None if `cancelled` became true first.
    fn synthesize(&mut self, text: &str, rate: u32, cancelled: &dyn Fn() -> bool) -> Result<Option<Vec<f32>>>;

    /// What it is, for the terminal and the record.
    fn name(&self) -> String;
}

pub struct Say {
    voice: Option<String>,
    /// Words per minute; None for the voice's own.
    words_per_minute: Option<u32>,
    #[cfg(target_os = "macos")]
    dir: std::path::PathBuf,
    #[cfg(target_os = "macos")]
    count: u64,
}

#[cfg(target_os = "macos")]
impl Say {
    pub fn new(voice: Option<String>, words_per_minute: Option<u32>) -> Result<Self> {
        use anyhow::{Context as _, bail};
        if let Some(v) = &voice {
            let list = std::process::Command::new("say")
                .args(["-v", "?"])
                .output()
                .context("could not run `say`, the Mac's text to speech")?;
            let list = String::from_utf8_lossy(&list.stdout);
            // Lines like "Samantha            en_US    # Hello!" or
            // "Reed (English (US)) en_US    # …".
            let known = list.lines().any(|l| {
                l.split('#')
                    .next()
                    .and_then(|name| name.trim_end().rsplit_once(' '))
                    .is_some_and(|(name, _)| name.trim() == v)
            });
            if !known {
                bail!(
                    "there is no voice {v:?}: `say -v '?'` lists them (add more in System Settings > \
                     Accessibility > Spoken Content)"
                );
            }
        }
        let dir = std::env::temp_dir().join(format!("meeting-transcriber-agent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).with_context(|| format!("could not create {}", dir.display()))?;
        Ok(Self {
            voice,
            words_per_minute,
            dir,
            count: 0,
        })
    }
}

#[cfg(not(target_os = "macos"))]
impl Say {
    pub fn new(voice: Option<String>, words_per_minute: Option<u32>) -> Result<Self> {
        let _ = (voice, words_per_minute);
        anyhow::bail!(
            "the voice agent speaks with the Mac's `say` and has no voice on {} yet",
            std::env::consts::OS
        )
    }
}

#[cfg(target_os = "macos")]
impl Drop for Say {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Synth for Say {
    #[cfg(target_os = "macos")]
    fn synthesize(&mut self, text: &str, rate: u32, cancelled: &dyn Fn() -> bool) -> Result<Option<Vec<f32>>> {
        use std::io::Write as _;
        use std::process::{Command, Stdio};

        use anyhow::{Context as _, bail};

        self.count += 1;
        let path = self.dir.join(format!("{}.wav", self.count));
        let mut cmd = Command::new("say");
        if let Some(v) = &self.voice {
            cmd.args(["-v", v]);
        }
        if let Some(wpm) = self.words_per_minute {
            cmd.args(["-r", &wpm.to_string()]);
        }
        // The text on standard input: nothing in it is taken for an option.
        cmd.args(["-f", "-", "-o"])
            .arg(&path)
            .args(["--file-format=WAVE", &format!("--data-format=LEI16@{rate}")])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().context("could not run `say`")?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(text.as_bytes())
                .context("could not hand the text to `say`")?;
        }
        let status = loop {
            if cancelled() {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_file(&path);
                return Ok(None);
            }
            if let Some(status) = child.try_wait()? {
                break status;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        if !status.success() {
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                let _ = std::io::Read::read_to_string(&mut e, &mut err);
            }
            bail!("`say` failed ({status}): {}", err.trim());
        }
        let samples = read_wav(&path);
        let _ = std::fs::remove_file(&path);
        Ok(Some(samples?))
    }

    #[cfg(not(target_os = "macos"))]
    fn synthesize(&mut self, _: &str, _: u32, _: &dyn Fn() -> bool) -> Result<Option<Vec<f32>>> {
        anyhow::bail!("no voice on this system")
    }

    fn name(&self) -> String {
        let voice = self.voice.as_deref().unwrap_or("system voice");
        match self.words_per_minute {
            Some(wpm) => format!("macOS say ({voice}, {wpm} words a minute)"),
            None => format!("macOS say ({voice})"),
        }
    }
}

/// A 16-bit mono WAV file's samples.
#[cfg(target_os = "macos")]
fn read_wav(path: &std::path::Path) -> Result<Vec<f32>> {
    use anyhow::Context as _;
    let mut reader = hound::WavReader::open(path).with_context(|| format!("could not read {}", path.display()))?;
    reader
        .samples::<i16>()
        .map(|s| s.map(|s| s as f32 / 32768.0).map_err(Into::into))
        .collect()
}
