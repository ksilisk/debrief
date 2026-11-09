//! Speech to text with Whisper (whisper.cpp), the model files it needs and the
//! names and terms it gets as hints.

pub mod hints;
pub mod models;
mod whisper;

pub use whisper::{LoadError, Whisper};
