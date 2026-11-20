use lna_llm::adapters::adapter_for;
use lna_llm::sse::SseParser;
use lna_llm::{LlmEvent, StopReason};
use lna_types::ProtocolKind;

fn replay_fixture(kind: ProtocolKind, raw: &str) -> Vec<LlmEvent> {
    let adapter = adapter_for(kind);
    let mut decoder = adapter.decoder("test-model");
    let mut parser = SseParser::new();
    let mut events = Vec::new();
    // Feed in small 17-byte chunks to verify UTF-8 & frame boundary handling
    for chunk in raw.as_bytes().chunks(17) {
        for frame in parser.push(chunk).unwrap() {
            events.extend(decoder.on_frame(frame).unwrap());
        }
    }
    if let Some(last) = parser.finish() {
        events.extend(decoder.on_frame(last).unwrap());
    }
    events.extend(decoder.on_eof().unwrap());
    events
}

#[test]
fn replay_all_four_protocols() {
    let cc = replay_fixture(
        ProtocolKind::ChatCompletion,
        include_str!("../../../tests/fixtures/chat_completion_replay.sse"),
    );
    assert!(cc.iter().any(|e| matches!(e, LlmEvent::ThoughtDelta(_))));
    assert!(cc
        .iter()
        .any(|e| matches!(e, LlmEvent::ToolCallStart { .. })));
    assert!(cc.iter().any(|e| matches!(
        e,
        LlmEvent::Finished {
            reason: StopReason::ToolUse
        }
    )));

    let resp = replay_fixture(
        ProtocolKind::Responses,
        include_str!("../../../tests/fixtures/responses_replay.sse"),
    );
    assert!(resp.iter().any(|e| matches!(e, LlmEvent::Opaque(_))));
    assert!(resp.iter().any(|e| matches!(
        e,
        LlmEvent::Finished {
            reason: StopReason::EndTurn
        }
    )));

    let ant = replay_fixture(
        ProtocolKind::Anthropic,
        include_str!("../../../tests/fixtures/anthropic_replay.sse"),
    );
    assert!(ant.iter().any(|e| matches!(e, LlmEvent::Opaque(_))));
    assert!(ant
        .iter()
        .any(|e| matches!(e, LlmEvent::TextDelta(t) if t.contains("Hello"))));

    let gem = replay_fixture(
        ProtocolKind::Gemini,
        include_str!("../../../tests/fixtures/gemini_replay.sse"),
    );
    assert!(gem.iter().any(|e| matches!(e, LlmEvent::Opaque(_))));
    assert!(gem
        .iter()
        .any(|e| matches!(e, LlmEvent::Usage(u) if u.input_tokens == 20)));
}
