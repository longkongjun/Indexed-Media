use serde::Serialize;

/// 文件因明确辅助视频标记而不进入识别流水线的稳定原因。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuxiliaryReason {
    /// 样片或片段。
    Sample,
    /// 预告片。
    Trailer,
    /// 花絮、特辑或额外内容。
    Extra,
}

impl AuxiliaryReason {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Sample => "sample",
            Self::Trailer => "trailer",
            Self::Extra => "extra",
        }
    }
}

/// 仅根据显式路径/文件名标记识别辅助视频。
///
/// 文件大小和时长不参与判定。文件名标记必须是最后一个独立 token，因此
/// `Sample (2019)`、`Trailer Park Boys` 等正式标题不会因包含关键词而被跳过。
#[must_use]
pub fn classify_auxiliary(path: &str) -> Option<AuxiliaryReason> {
    let normalized = path.replace('\\', "/");
    let mut components = normalized
        .split('/')
        .filter(|value| !value.is_empty())
        .peekable();
    let mut file_name = "";
    while let Some(component) = components.next() {
        if components.peek().is_some() {
            if let Some(reason) = directory_reason(component) {
                return Some(reason);
            }
        } else {
            file_name = component;
        }
    }
    let stem = file_name
        .rsplit_once('.')
        .map_or(file_name, |(stem, _)| stem);
    let tokens = stem
        .split(|character: char| {
            character.is_whitespace()
                || matches!(
                    character,
                    '.' | '_' | '-' | '(' | ')' | '[' | ']' | '{' | '}'
                )
        })
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    if tokens.len() < 2 {
        return None;
    }
    tokens.last().and_then(|token| token_reason(token))
}

fn directory_reason(value: &str) -> Option<AuxiliaryReason> {
    match value.trim().to_lowercase().as_str() {
        "sample" | "samples" => Some(AuxiliaryReason::Sample),
        "trailer" | "trailers" | "预告" | "预告片" => Some(AuxiliaryReason::Trailer),
        "extra" | "extras" | "featurette" | "featurettes" | "花絮" | "特辑" => {
            Some(AuxiliaryReason::Extra)
        }
        _ => None,
    }
}

fn token_reason(value: &str) -> Option<AuxiliaryReason> {
    match value {
        "sample" => Some(AuxiliaryReason::Sample),
        "trailer" | "预告" | "预告片" => Some(AuxiliaryReason::Trailer),
        "extra" | "extras" | "featurette" | "花絮" | "特辑" => Some(AuxiliaryReason::Extra),
        _ => None,
    }
}
