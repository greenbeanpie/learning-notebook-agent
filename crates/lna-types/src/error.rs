use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    InvalidArgument,
    NotFound,
    Conflict,
    Cancelled,
    NoSourceSelected,
    SourceLimitReached,
    SourceTooLarge,
    SourceUnsupported,
    SourceEncrypted,
    SourceNeedsOcr,
    UrlBlocked,
    FetchFailed,
    ModelNotConfigured,
    ModelAuthFailed,
    ModelNotFound,
    ModelRateLimited,
    ModelOverloaded,
    ModelBadRequest,
    ModelContextExceeded,
    ModelRefused,
    ModelStreamInterrupted,
    OutputTruncated,
    OutputInvalid,
    CapabilityMissing,
    KeychainUnavailable,
    StorageFull,
    Internal,
}

impl ErrorCode {
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::ModelRateLimited
                | Self::ModelOverloaded
                | Self::ModelStreamInterrupted
                | Self::FetchFailed
        )
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InvalidArgument => "INVALID_ARGUMENT",
            Self::NotFound => "NOT_FOUND",
            Self::Conflict => "CONFLICT",
            Self::Cancelled => "CANCELLED",
            Self::NoSourceSelected => "NO_SOURCE_SELECTED",
            Self::SourceLimitReached => "SOURCE_LIMIT_REACHED",
            Self::SourceTooLarge => "SOURCE_TOO_LARGE",
            Self::SourceUnsupported => "SOURCE_UNSUPPORTED",
            Self::SourceEncrypted => "SOURCE_ENCRYPTED",
            Self::SourceNeedsOcr => "SOURCE_NEEDS_OCR",
            Self::UrlBlocked => "URL_BLOCKED",
            Self::FetchFailed => "FETCH_FAILED",
            Self::ModelNotConfigured => "MODEL_NOT_CONFIGURED",
            Self::ModelAuthFailed => "MODEL_AUTH_FAILED",
            Self::ModelNotFound => "MODEL_NOT_FOUND",
            Self::ModelRateLimited => "MODEL_RATE_LIMITED",
            Self::ModelOverloaded => "MODEL_OVERLOADED",
            Self::ModelBadRequest => "MODEL_BAD_REQUEST",
            Self::ModelContextExceeded => "MODEL_CONTEXT_EXCEEDED",
            Self::ModelRefused => "MODEL_REFUSED",
            Self::ModelStreamInterrupted => "MODEL_STREAM_INTERRUPTED",
            Self::OutputTruncated => "OUTPUT_TRUNCATED",
            Self::OutputInvalid => "OUTPUT_INVALID",
            Self::CapabilityMissing => "CAPABILITY_MISSING",
            Self::KeychainUnavailable => "KEYCHAIN_UNAVAILABLE",
            Self::StorageFull => "STORAGE_FULL",
            Self::Internal => "INTERNAL",
        }
    }
}

#[derive(Debug, Error, Clone, Serialize, Deserialize)]
#[error("[{code:?}] {message}")]
pub struct LnaError {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl LnaError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: code.is_retryable(),
            details: None,
        }
    }

    pub fn with_retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }

    pub fn invalid(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArgument, msg)
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, msg)
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, msg)
    }
}

pub type LnaResult<T> = Result<T, LnaError>;
