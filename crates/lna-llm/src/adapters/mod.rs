pub mod anthropic;
pub mod chat_completion;
pub mod gemini;
pub mod responses;

use crate::types::ProtocolAdapter;
use lna_types::ProtocolKind;

pub fn adapter_for(kind: ProtocolKind) -> Box<dyn ProtocolAdapter> {
    match kind {
        ProtocolKind::ChatCompletion => Box::new(chat_completion::ChatCompletionAdapter),
        ProtocolKind::Responses => Box::new(responses::ResponsesAdapter),
        ProtocolKind::Anthropic => Box::new(anthropic::AnthropicAdapter),
        ProtocolKind::Gemini => Box::new(gemini::GeminiAdapter),
    }
}
