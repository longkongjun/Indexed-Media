use chrono::NaiveDate;

use crate::connectors::model::{
    CacheStatus, CandidateIdentity, FieldLanguageSource, LocalizedField, MetadataCandidate,
    ProviderError, ProviderMediaKind,
};
use crate::connectors::tmdb::dto::WorkDto;

pub(super) const PROVIDER_VERSION: u16 = 1;
const MAX_TITLE_CHARS: usize = 512;
const MAX_SUMMARY_CHARS: usize = 4096;
const MAX_LOCALIZED_VALUES: usize = 8;

pub(super) fn map_work(
    dto: &WorkDto,
    media_kind: ProviderMediaKind,
    language: &str,
    source: FieldLanguageSource,
    now_us: i64,
) -> Result<MetadataCandidate, ProviderError> {
    if dto.id <= 0 || !valid_locale(language) {
        return Err(ProviderError::InvalidResponse);
    }
    let display_title = match media_kind {
        ProviderMediaKind::Movie => dto.title.as_deref().or(dto.original_title.as_deref()),
        ProviderMediaKind::Tv => dto.name.as_deref().or(dto.original_name.as_deref()),
    }
    .ok_or(ProviderError::InvalidResponse)?;
    let display_title = bounded_text(display_title, MAX_TITLE_CHARS)?;
    let date = match media_kind {
        ProviderMediaKind::Movie => dto.release_date.as_deref(),
        ProviderMediaKind::Tv => dto.first_air_date.as_deref(),
    }
    .filter(|value| !value.is_empty())
    .map(|value| {
        NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| ProviderError::InvalidResponse)
    })
    .transpose()?;
    let summary = dto
        .overview
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(|value| bounded_text(value, MAX_SUMMARY_CHARS))
        .transpose()?;
    if dto.original_language.as_deref().is_some_and(|value| {
        value.is_empty()
            || value.len() > 8
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
    }) {
        return Err(ProviderError::InvalidResponse);
    }
    Ok(MetadataCandidate {
        identity: CandidateIdentity {
            provider_id: dto.id,
            media_kind,
        },
        titles: vec![LocalizedField {
            value: display_title,
            language: language.to_owned(),
            source,
        }],
        summaries: summary
            .map(|value| {
                vec![LocalizedField {
                    value,
                    language: language.to_owned(),
                    source,
                }]
            })
            .unwrap_or_default(),
        release_dates: date
            .map(|value| {
                vec![LocalizedField {
                    value,
                    language: language.to_owned(),
                    source,
                }]
            })
            .unwrap_or_default(),
        aliases: Vec::new(),
        episodes: Vec::new(),
        provider_version: PROVIDER_VERSION,
        cache_status: CacheStatus::Miss,
        retrieved_at_us: now_us,
    })
}

pub(super) fn merge_localized(
    target: &mut MetadataCandidate,
    source: MetadataCandidate,
) -> Result<(), ProviderError> {
    if target.identity != source.identity {
        return Err(ProviderError::InvalidResponse);
    }
    merge_unique(&mut target.titles, source.titles, MAX_LOCALIZED_VALUES);
    merge_unique(
        &mut target.summaries,
        source.summaries,
        MAX_LOCALIZED_VALUES,
    );
    merge_unique(
        &mut target.release_dates,
        source.release_dates,
        MAX_LOCALIZED_VALUES,
    );
    Ok(())
}

fn merge_unique<T: Eq>(
    target: &mut Vec<LocalizedField<T>>,
    incoming: Vec<LocalizedField<T>>,
    maximum: usize,
) {
    for field in incoming {
        if target.len() >= maximum {
            break;
        }
        if !target
            .iter()
            .any(|existing| existing.value == field.value && existing.language == field.language)
        {
            target.push(field);
        }
    }
}

fn bounded_text(value: &str, maximum: usize) -> Result<String, ProviderError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > maximum || value.chars().any(char::is_control) {
        Err(ProviderError::InvalidResponse)
    } else {
        Ok(value.to_owned())
    }
}

pub(super) fn valid_locale(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 5
        && bytes[..2].iter().all(u8::is_ascii_lowercase)
        && bytes[2] == b'-'
        && bytes[3..].iter().all(u8::is_ascii_uppercase)
}

pub(super) fn original_locale(language: &str) -> Option<String> {
    let region = match language {
        "en" => "US",
        "ja" => "JP",
        "ko" => "KR",
        "zh" => "CN",
        "fr" => "FR",
        "de" => "DE",
        "es" => "ES",
        "it" => "IT",
        "pt" => "PT",
        _ => return None,
    };
    Some(format!("{language}-{region}"))
}
