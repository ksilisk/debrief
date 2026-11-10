//! Speech to text with Whisper (whisper.cpp): the speech detector that decides
//! what Whisper hears, the model files and the names and terms it gets as hints.

pub mod hints;
mod invented;
pub mod models;
mod speech;
mod whisper;

pub use whisper::{LoadError, Whisper};
