//! Error type for the Electrobun SDK.
//!
//! Mirrors the Zig SDK's `error.ElectrobunCoreFailure` / `error.MissingCoreSymbol`
//! family. Every mutating Core call polls `electrobun_core_last_error`; a non-empty
//! message becomes an [`ElectrobunError::Core`].

use std::fmt;

/// Errors surfaced by the SDK.
#[derive(Debug)]
pub enum ElectrobunError {
    /// `libElectrobunCore` (or `libwebgpu_dawn`) could not be opened.
    LibraryOpen {
        /// Path the SDK attempted to `dlopen`.
        path: String,
        /// Underlying loader error.
        source: libloading::Error,
    },
    /// A required C-ABI symbol was absent from the loaded library.
    MissingCoreSymbol {
        /// The unresolved symbol name.
        symbol: &'static str,
    },
    /// `electrobun_core_last_error` reported a non-empty message after a call,
    /// or a call returned the documented failure sentinel (id `0` / null).
    Core(String),
    /// The executable path could not be resolved to a bundle layout.
    InvalidExePath,
    /// A filesystem read failed (e.g. `version.json`, preload scripts).
    Io {
        /// What was being read.
        context: String,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// A JSON payload returned by core (or read from the bundle) was malformed.
    Json {
        /// What was being parsed.
        context: String,
        /// Underlying serde error.
        source: serde_json::Error,
    },
    /// A required environment variable was unset and had no fallback.
    MissingEnv {
        /// The variable name.
        name: &'static str,
    },
}

impl fmt::Display for ElectrobunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LibraryOpen { path, source } => {
                write!(f, "failed to open library at {path}: {source}")
            }
            Self::MissingCoreSymbol { symbol } => {
                write!(f, "missing core symbol: {symbol}")
            }
            Self::Core(message) => write!(f, "electrobun core error: {message}"),
            Self::InvalidExePath => write!(f, "could not resolve executable bundle path"),
            Self::Io { context, source } => write!(f, "i/o error ({context}): {source}"),
            Self::Json { context, source } => write!(f, "json error ({context}): {source}"),
            Self::MissingEnv { name } => write!(f, "environment variable not set: {name}"),
        }
    }
}

impl std::error::Error for ElectrobunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::LibraryOpen { source, .. } => Some(source),
            Self::Io { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Convenience alias used throughout the SDK.
pub type Result<T> = std::result::Result<T, ElectrobunError>;
