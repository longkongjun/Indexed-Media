use serde::Deserialize;

use super::super::model::{EnhancementHints, EnhancerMediaKind};
use super::super::port::{EnhancerError, EnhancerProbe};
use super::dto::{ChatResponse, TagsResponse};

pub(super) fn probe(body: &[u8], expected_model: &str) -> Result<EnhancerProbe, EnhancerError> {
    let response: TagsResponse =
        serde_json::from_slice(body).map_err(|_| EnhancerError::InvalidResponse)?;
    if response.models.len() > 1_000
        || response.models.iter().any(|model| {
            !valid_protocol_text(&model.name, 128)
                || model
                    .model
                    .as_deref()
                    .is_some_and(|value| !valid_protocol_text(value, 128))
                || model
                    .modified_at
                    .as_deref()
                    .is_some_and(|value| !valid_protocol_text(value, 64))
                || model
                    .digest
                    .as_deref()
                    .is_some_and(|value| !valid_protocol_text(value, 256))
                || model.size.is_some_and(|value| value > i64::MAX as u64)
                || model.details.as_ref().is_some_and(invalid_details)
        })
    {
        return Err(EnhancerError::InvalidResponse);
    }
    let model_available = response.models.iter().any(|model| {
        model.name == expected_model || model.model.as_deref() == Some(expected_model)
    });
    Ok(EnhancerProbe {
        adapter_version: "ollama-v1".to_owned(),
        model_available,
    })
}

pub(super) fn hints(body: &[u8], expected_model: &str) -> Result<EnhancementHints, EnhancerError> {
    let response: ChatResponse =
        serde_json::from_slice(body).map_err(|_| EnhancerError::InvalidResponse)?;
    if response.model != expected_model
        || !response.done
        || response.message.role != "assistant"
        || response.message.content.len() > 16 * 1024
        || response
            .created_at
            .as_deref()
            .is_some_and(|value| !valid_protocol_text(value, 64))
        || response
            .done_reason
            .as_deref()
            .is_some_and(|value| !valid_protocol_text(value, 64))
        || duration_overflow(&response)
    {
        return Err(EnhancerError::InvalidResponse);
    }
    let wire: HintsWire = serde_json::from_str(&response.message.content)
        .map_err(|_| EnhancerError::InvalidResponse)?;
    EnhancementHints {
        title: wire.title,
        year: wire.year,
        media_kind: wire.media_kind,
        season: wire.season,
        episodes: wire.episodes,
    }
    .validate()
}

fn duration_overflow(response: &ChatResponse) -> bool {
    [
        response.total_duration,
        response.load_duration,
        response.prompt_eval_count,
        response.prompt_eval_duration,
        response.eval_count,
        response.eval_duration,
    ]
    .into_iter()
    .flatten()
    .any(|value| value > i64::MAX as u64)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HintsWire {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    year: Option<u16>,
    #[serde(default)]
    media_kind: Option<EnhancerMediaKind>,
    #[serde(default)]
    season: Option<u16>,
    #[serde(default)]
    episodes: Vec<u16>,
}

fn invalid_details(details: &super::dto::ModelDetails) -> bool {
    let scalar = [
        details.parent_model.as_deref(),
        details.format.as_deref(),
        details.family.as_deref(),
        details.parameter_size.as_deref(),
        details.quantization_level.as_deref(),
    ];
    scalar
        .into_iter()
        .flatten()
        .any(|value| !valid_protocol_text(value, 128))
        || details.families.as_ref().is_some_and(|families| {
            families.len() > 32
                || families
                    .iter()
                    .any(|value| !valid_protocol_text(value, 128))
        })
}

fn valid_protocol_text(value: &str, max_chars: usize) -> bool {
    (1..=max_chars).contains(&value.chars().count()) && !value.chars().any(char::is_control)
}
