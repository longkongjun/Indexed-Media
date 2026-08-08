use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::super::model::EnhancementInput;
use super::super::port::EnhancerError;

const SYSTEM_INSTRUCTION: &str = "Return only JSON matching the supplied schema. Infer conservative supporting media identity hints from the basename and at most two parent segments. Never return provider identifiers, paths, commands, or explanations.";
const MAX_REQUEST_BYTES: usize = 16 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TagsResponse {
    pub models: Vec<TagModel>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TagModel {
    pub name: String,
    pub model: Option<String>,
    pub modified_at: Option<String>,
    pub size: Option<u64>,
    pub digest: Option<String>,
    pub details: Option<ModelDetails>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ModelDetails {
    pub parent_model: Option<String>,
    pub format: Option<String>,
    pub family: Option<String>,
    pub families: Option<Vec<String>>,
    pub parameter_size: Option<String>,
    pub quantization_level: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChatResponse {
    pub model: String,
    pub created_at: Option<String>,
    pub message: ChatResponseMessage,
    pub done: bool,
    pub done_reason: Option<String>,
    pub total_duration: Option<u64>,
    pub load_duration: Option<u64>,
    pub prompt_eval_count: Option<u64>,
    pub prompt_eval_duration: Option<u64>,
    pub eval_count: Option<u64>,
    pub eval_duration: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ChatResponseMessage {
    pub role: String,
    pub content: String,
}

#[derive(Serialize)]
struct UserInput<'a> {
    basename: &'a str,
    parent_segments: &'a [String],
    base_hint: &'a super::super::model::BaseIdentityHint,
}

pub(super) fn request_body(
    model: &str,
    input: &EnhancementInput,
) -> Result<Vec<u8>, EnhancerError> {
    let user = serde_json::to_string(&UserInput {
        basename: input.basename(),
        parent_segments: input.parent_segments(),
        base_hint: input.base_hint(),
    })
    .map_err(|_| EnhancerError::InvalidResponse)?;
    let request = json!({
        "model": model,
        "messages": [
            {"role":"system","content":SYSTEM_INSTRUCTION},
            {"role":"user","content":user}
        ],
        "stream": false,
        "format": output_schema(),
        "options": {"temperature":0,"seed":0}
    });
    let body = serde_json::to_vec(&request).map_err(|_| EnhancerError::InvalidResponse)?;
    if body.len() > MAX_REQUEST_BYTES {
        return Err(EnhancerError::InvalidResponse);
    }
    Ok(body)
}

fn output_schema() -> Value {
    json!({
        "type":"object",
        "additionalProperties":false,
        "properties":{
            "title":{"type":["string","null"],"maxLength":512},
            "year":{"type":["integer","null"],"minimum":1878,"maximum":2100},
            "media_kind":{"type":["string","null"],"enum":["movie","episode",null]},
            "season":{"type":["integer","null"],"minimum":1,"maximum":10000},
            "episodes":{"type":"array","maxItems":100,"items":{"type":"integer","minimum":1,"maximum":10000}}
        },
        "required":["title","year","media_kind","season","episodes"]
    })
}
