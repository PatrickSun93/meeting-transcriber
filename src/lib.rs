//! Live meeting transcription for macOS, Windows and Linux. Your microphone
//! is "Me", everything the computer plays is "Others", and a local Whisper
//! model writes it down. The same model also does hold-to-talk dictation.

pub mod app;
pub mod audio;
pub mod chunker;
pub mod dictation;
pub mod meeting;
pub mod meetings;
pub mod models;
pub mod platform;
pub mod session;
pub mod settings;
pub mod transcriber;
pub mod transcript;
pub mod wav;
