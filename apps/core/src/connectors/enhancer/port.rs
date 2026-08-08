use async_trait::async_trait;

pub use super::model::EnhancementHints;
use super::model::EnhancementInput;

#[derive(Clone, Copy)]
/// Borrowed validated endpoint configuration for one bounded protocol call.
pub struct EnhancerEndpoint<'a> {
    pub base_url: &'a str,
    pub model: &'a str,
    pub timeout_ms: u32,
}

impl std::fmt::Debug for EnhancerEndpoint<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("EnhancerEndpoint")
            .field("base_url", &"[REDACTED]")
            .field("model", &"[REDACTED]")
            .field("timeout_ms", &self.timeout_ms)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Safe capability result from the fixed Ollama adapter.
pub struct EnhancerProbe {
    pub adapter_version: String,
    pub model_available: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
/// Stable local enhancer failure without endpoint, prompt, or response text.
pub enum EnhancerError {
    #[error("identification enhancer is disabled")]
    Disabled,
    #[error("identification enhancer model is unavailable")]
    ModelUnavailable,
    #[error("identification enhancer is overloaded")]
    Overloaded,
    #[error("identification enhancer is unavailable")]
    Unavailable,
    #[error("identification enhancer timed out")]
    Timeout,
    #[error("identification enhancer response is too large")]
    ResponseTooLarge,
    #[error("identification enhancer response is invalid")]
    InvalidResponse,
}

impl EnhancerError {
    #[must_use]
    /// Stable redacted category used by fallback evidence.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::ModelUnavailable => "model-unavailable",
            Self::Overloaded => "overloaded",
            Self::Unavailable => "unavailable",
            Self::Timeout => "timeout",
            Self::ResponseTooLarge => "response-too-large",
            Self::InvalidResponse => "invalid-response",
        }
    }
}

#[async_trait]
/// Closed local identification enhancer protocol boundary.
pub trait IdentificationEnhancer: Send + Sync {
    async fn probe(&self, endpoint: EnhancerEndpoint<'_>) -> Result<EnhancerProbe, EnhancerError>;

    async fn enhance(
        &self,
        endpoint: EnhancerEndpoint<'_>,
        input: &EnhancementInput,
    ) -> Result<EnhancementHints, EnhancerError>;
}
