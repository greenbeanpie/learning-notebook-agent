//! The user's own material (pictures attached to a Studio request) as the model sees it.
//!
//! A model that can see images gets the pictures themselves in the request, not a text
//! transcription made by another call: the transcription loses formulas and figures, and any
//! stage that forgot to include it was blind. A model that cannot see images keeps the text
//! description (`image_brief`), which every stage then has to carry.

use lna_ingest::attachments;
use lna_llm::{LlmMessage, LlmRequest, Part, ResolvedProfile, Role};
use lna_types::StudioArtifact;
use serde_json::{json, Value};
use std::path::Path;

/// Image parts for the pictures the user attached to `art`, when `resolved` can see images;
/// empty otherwise (such a model reads the text description instead).
pub(crate) fn image_parts(
    root: &Path,
    art: &StudioArtifact,
    resolved: &ResolvedProfile,
) -> Vec<Part> {
    if !resolved.capabilities.supports_vision {
        return Vec::new();
    }
    let Some(ids) = art.config.get("images") else {
        return Vec::new();
    };
    let pics = attachments::images(&attachments::resolve(root, ids));
    if pics.is_empty() {
        return Vec::new();
    }
    attachments::image_parts(&pics)
}

/// `req` with the pictures placed before the text of its first user message.
pub(crate) fn with_images(mut req: LlmRequest, images: &[Part]) -> LlmRequest {
    if images.is_empty() {
        return req;
    }
    if let Some(first) = req.messages.iter_mut().find(|m| m.role == Role::User) {
        let mut parts = images.to_vec();
        parts.append(&mut first.parts);
        first.parts = parts;
    } else {
        req.messages.insert(
            0,
            LlmMessage {
                role: Role::User,
                parts: images.to_vec(),
            },
        );
    }
    req
}

/// The artifact's `config` as printed into prompts: attachment ids mean nothing to the model
/// (and made it hunt for tools that open them), so they become a plain statement.
pub(crate) fn config_for_prompt(config: &Value, pictures_shown: bool) -> Value {
    let mut c = config.clone();
    if let Some(obj) = c.as_object_mut() {
        let n_images = obj
            .remove("images")
            .and_then(|v| v.as_array().map(|a| a.len()))
            .unwrap_or(0);
        let n_files = obj
            .remove("files")
            .and_then(|v| v.as_array().map(|a| a.len()))
            .unwrap_or(0);
        let n_refs = obj
            .remove("ref_artifact_ids")
            .and_then(|v| v.as_array().map(|a| a.len()))
            .unwrap_or(0);
        if n_images > 0 {
            obj.insert(
                "attached_images".into(),
                json!(if pictures_shown {
                    format!("{n_images} picture(s), shown in this message")
                } else {
                    format!("{n_images} picture(s), described in <user_images>")
                }),
            );
        }
        if n_files > 0 {
            obj.insert(
                "attached_files".into(),
                json!(format!("{n_files} file(s), summarised in <user_files>")),
            );
        }
        if n_refs > 0 {
            obj.insert(
                "referenced_artifacts".into(),
                json!(format!("{n_refs} Studio artifact(s), included in <referenced_artifacts>")),
            );
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use lna_llm::{OutputMode, ReasoningOptions, ToolMode};

    fn req(role_first: Role) -> LlmRequest {
        LlmRequest {
            system: String::new(),
            messages: vec![LlmMessage {
                role: role_first,
                parts: vec![Part::Text("question".into())],
            }],
            tools: vec![],
            tool_mode: ToolMode::None,
            output: OutputMode::Text,
            max_output_tokens: 0,
            reasoning: ReasoningOptions::default(),
        }
    }

    fn media() -> Part {
        Part::Media {
            mime_type: "image/png".into(),
            data_base64: Some("AAAA".into()),
            file_uri: None,
            audio_format: None,
        }
    }

    #[test]
    fn pictures_go_before_the_first_user_text() {
        let r = with_images(req(Role::User), &[media()]);
        assert!(matches!(r.messages[0].parts[0], Part::Media { .. }));
        assert!(matches!(r.messages[0].parts[1], Part::Text(_)));
        let untouched = with_images(req(Role::User), &[]);
        assert_eq!(untouched.messages[0].parts.len(), 1);
    }

    #[test]
    fn attachment_ids_never_reach_the_prompt() {
        let cfg = json!({"topic": "t", "images": ["01ABC", "01DEF"], "files": ["01XYZ"], "ref_artifact_ids": ["01ART"]});
        let shown = config_for_prompt(&cfg, true).to_string();
        assert!(
            !shown.contains("01ABC") && !shown.contains("01XYZ") && !shown.contains("01ART"),
            "{shown}"
        );
        assert!(shown.contains("1 Studio artifact(s)"), "{shown}");
        assert!(
            shown.contains("2 picture(s), shown in this message"),
            "{shown}"
        );
        let described = config_for_prompt(&cfg, false).to_string();
        assert!(described.contains("<user_images>"), "{described}");
        assert_eq!(
            config_for_prompt(&json!({"topic": "t"}), true),
            json!({"topic": "t"})
        );
    }
}
