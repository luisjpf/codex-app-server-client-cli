use std::fmt;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode as StdExitCode;

use thiserror::Error;

use crate::approval::ApprovalRequiredEnvelope;
use crate::cli::Transport;

#[derive(Debug, Clone, Copy)]
pub enum ExitCode {
    Success = 0,
    InternalFailure = 1,
    Usage = 2,
    Connection = 3,
    Protocol = 4,
    Interrupted = 5,
    LocalConfig = 6,
    ApprovalRequired = 7,
}

impl From<ExitCode> for StdExitCode {
    fn from(value: ExitCode) -> Self {
        StdExitCode::from(value as u8)
    }
}

impl fmt::Display for ExitCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", *self as u8)
    }
}

#[derive(Debug, Error)]
pub enum AppError {
    #[error("config I/O failed for {path}: {source}")]
    ConfigIo {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("config parse failed for {path}: {source}")]
    ConfigParse {
        path: PathBuf,
        source: toml::de::Error,
    },

    #[error("stdout write failed: {0}")]
    Stdout(io::Error),

    #[error("json serialization failed: {0}")]
    Json(serde_json::Error),

    #[error("tracing initialization failed: {0}")]
    TracingInit(String),

    #[error("unsupported transport {transport:?}: {detail}")]
    UnsupportedTransport {
        transport: Transport,
        detail: String,
    },

    #[error("connection failure during {phase}: {detail}")]
    Connection { phase: &'static str, detail: String },

    #[error("authentication failure during {phase}: {detail}")]
    Authentication { phase: &'static str, detail: String },

    #[error("protocol failure during {phase}: {detail}")]
    Protocol { phase: &'static str, detail: String },

    #[error("approval required: {message}")]
    ApprovalRequired {
        envelope: Box<ApprovalRequiredEnvelope>,
        message: String,
    },
}

impl AppError {
    pub fn config_io(path: PathBuf, source: io::Error) -> Self {
        Self::ConfigIo { path, source }
    }

    pub fn config_parse(path: PathBuf, source: toml::de::Error) -> Self {
        Self::ConfigParse { path, source }
    }

    pub fn stdout(source: io::Error) -> Self {
        Self::Stdout(source)
    }

    pub fn json(source: serde_json::Error) -> Self {
        Self::Json(source)
    }

    pub fn unsupported_transport(transport: Transport, detail: impl Into<String>) -> Self {
        Self::UnsupportedTransport {
            transport,
            detail: detail.into(),
        }
    }

    pub fn connection(phase: &'static str, detail: impl Into<String>) -> Self {
        Self::Connection {
            phase,
            detail: detail.into(),
        }
    }

    pub fn authentication(phase: &'static str, detail: impl Into<String>) -> Self {
        Self::Authentication {
            phase,
            detail: detail.into(),
        }
    }

    pub fn protocol(phase: &'static str, detail: impl Into<String>) -> Self {
        Self::Protocol {
            phase,
            detail: detail.into(),
        }
    }

    pub fn approval_required(envelope: ApprovalRequiredEnvelope) -> Self {
        let message = envelope.message.clone();
        Self::ApprovalRequired {
            envelope: Box::new(envelope),
            message,
        }
    }

    pub fn exit_code(&self) -> ExitCode {
        match self {
            AppError::ConfigIo { .. } | AppError::ConfigParse { .. } => ExitCode::LocalConfig,
            AppError::UnsupportedTransport { .. } | AppError::Protocol { .. } => ExitCode::Protocol,
            AppError::Connection { .. } | AppError::Authentication { .. } => ExitCode::Connection,
            AppError::ApprovalRequired { .. } => ExitCode::ApprovalRequired,
            AppError::Stdout(_) | AppError::Json(_) | AppError::TracingInit(_) => {
                ExitCode::InternalFailure
            }
        }
    }
}

impl From<AppError> for StdExitCode {
    fn from(value: AppError) -> Self {
        let exit_code = value.exit_code();
        if !matches!(value, AppError::ApprovalRequired { .. }) {
            eprintln!("{value}");
        }
        exit_code.into()
    }
}
