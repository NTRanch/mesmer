//! Typed tool-level failures.
//!
//! Handlers return `Err(MesmerError)` and the server renders it as
//! an `isError` tool result whose text payload is
//! `{"error":{"code","message"}}` — the fleet convention. Gate
//! refusals (`MESMER-*` codes from [`crate::gate::RefusalCode`])
//! travel this path too when a write is refused; an
//! `envelope_check` preview instead returns its refusals as data.

use std::fmt;

/// A domain failure with a stable machine-readable code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MesmerError {
    pub code: &'static str,
    pub message: String,
}

impl MesmerError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        MesmerError {
            code,
            message: message.into(),
        }
    }

    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new("invalid_input", message)
    }

    /// Render as the JSON text payload of an `isError` result.
    pub fn to_payload(&self) -> String {
        serde_json::json!({"error": {"code": self.code, "message": self.message}}).to_string()
    }
}

impl fmt::Display for MesmerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for MesmerError {}

pub type MesmerResult<T> = Result<T, MesmerError>;
