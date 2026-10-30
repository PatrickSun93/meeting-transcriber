//! Live meeting transcription for macOS. Your microphone is "Me", everything
//! the Mac plays is "Others", and a local Whisper model writes it down.

pub mod app;
pub mod audio;
pub mod chunker;
pub mod meeting;
pub mod meetings;
pub mod models;
pub mod session;
pub mod settings;
pub mod transcriber;
pub mod transcript;
pub mod wav;
