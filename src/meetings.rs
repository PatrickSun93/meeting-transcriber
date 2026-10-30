//! Where meeting transcripts are saved, and what the files are called.

use std::path::{Path, PathBuf};

/// `~/Documents/Meeting Transcripts`.
pub fn default_dir() -> PathBuf {
    dirs::document_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_default()
        .join("Meeting Transcripts")
}

/// `<dir>/Meeting 2026-10-29 14-05.txt` for the current local time.
pub fn new_file_path(dir: &Path) -> PathBuf {
    let stamp = chrono::Local::now().format("%Y-%m-%d %H-%M").to_string();
    unused_file_path(dir, &stamp)
}

/// `Meeting <stamp>.txt`, or `Meeting <stamp> (2).txt` and so on when two
/// meetings start in the same minute, so a transcript is never overwritten.
fn unused_file_path(dir: &Path, stamp: &str) -> PathBuf {
    let mut path = dir.join(format!("Meeting {stamp}.txt"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("Meeting {stamp} ({n}).txt"));
        n += 1;
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_minute_meetings_get_numbered() {
        let dir = std::env::temp_dir().join(format!("meeting-transcriber-test-{}-names", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let stamp = "2026-10-29 14-05";

        let first = unused_file_path(&dir, stamp);
        assert_eq!(first, dir.join("Meeting 2026-10-29 14-05.txt"));
        std::fs::write(&first, "").unwrap();
        let second = unused_file_path(&dir, stamp);
        assert_eq!(second, dir.join("Meeting 2026-10-29 14-05 (2).txt"));
        std::fs::write(&second, "").unwrap();
        assert_eq!(
            unused_file_path(&dir, stamp),
            dir.join("Meeting 2026-10-29 14-05 (3).txt")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
