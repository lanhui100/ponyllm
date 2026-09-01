//! ponyllm-protocol: Core protocol definitions for OpenAI Chat, OpenAI Responses, and Anthropic Messages.

pub mod common;
pub mod error;
pub mod openai;
pub mod anthropic;
pub mod translator;

pub use common::*;
pub use error::{ProtocolError, Result};
pub use translator::*;

