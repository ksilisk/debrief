//! Speech to text with Whisper (whisper.cpp): the speech detector that decides
//! what Whisper hears, and the model files.

mod invented;
pub mod models;
mod speech;
mod whisper;

pub use whisper::{LoadError, Whisper};
