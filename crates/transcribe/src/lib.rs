//! Speech to text with Whisper (whisper.cpp), and the model files it needs.

pub mod models;
mod whisper;

pub use whisper::{LoadError, Whisper};
