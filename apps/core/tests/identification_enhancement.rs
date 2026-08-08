use mediaflow_core::connectors::enhancer::model::{EnhancementHints, EnhancerMediaKind};
use mediaflow_core::connectors::model::{CandidateIdentity, ProviderMediaKind};
use mediaflow_core::identification::decision::{
    DecisionCandidate, DecisionInput, DecisionLevel, DecisionReason, DecisionRules, EnhancedField,
    EnhancementGuard, decide,
};
use mediaflow_core::identification::evidence::{EvidenceSource, EvidenceStrength};
use mediaflow_core::identification::model::{ExternalIdHint, MediaKind, ParsedIdentityHint};
use mediaflow_core::identification::service::apply_enhancement_hints;
use uuid::Uuid;

#[test]
fn valid_nonstandard_name_hints_are_supporting_and_never_provider_identity() {
    let base = hint(MediaKind::Movie, "release group mystery", None);
    let applied = apply_enhancement_hints(
        &base,
        &EnhancementHints {
            title: Some("Dune".to_owned()),
            year: Some(2021),
            media_kind: Some(EnhancerMediaKind::Movie),
            season: None,
            episodes: Vec::new(),
        },
        [7; 32],
    );

    assert_eq!(applied.hint.normalized_title, "Dune");
    assert_eq!(applied.hint.year, Some(2021));
    assert!(applied.hint.external_ids.is_empty());
    assert_eq!(applied.evidence.len(), 3);
    assert!(applied.evidence.iter().all(|item| {
        item.source == EvidenceSource::Enhancer
            && item.strength == EvidenceStrength::Supporting
            && item.source_version == "ollama-v1"
    }));
    assert!(applied.guard.relies_on_enhancer());
}

#[test]
fn model_only_strong_match_is_capped_at_probable_without_an_identity_graph() {
    let revision = Uuid::now_v7();
    let enhanced = hint(MediaKind::Movie, "Dune", Some(2021));
    let candidate = movie(438_631, "Dune", Some(2021));
    let guard = EnhancementGuard::from_fields([EnhancedField::Title, EnhancedField::Year]);

    let decision = decide(
        &DecisionInput::new(revision, &enhanced, std::slice::from_ref(&candidate))
            .with_enhancement_guard(guard),
        &DecisionRules::v1(),
    );

    assert_eq!(decision.level, DecisionLevel::Probable);
    assert_eq!(decision.selected_candidate, Some(candidate.id));
    assert!(decision.graph.is_none());
    assert!(
        decision
            .reasons
            .contains(&DecisionReason::EnhancerOnlyMatch)
    );
}

#[test]
fn verified_external_id_keeps_normal_confirmation_authority() {
    let revision = Uuid::now_v7();
    let mut enhanced = hint(MediaKind::Movie, "Dune", Some(2021));
    enhanced.external_ids.push(ExternalIdHint {
        provider: "tmdb".to_owned(),
        value: "438631".to_owned(),
        is_default: true,
    });
    let candidate = movie(438_631, "Dune", Some(2021));

    let decision = decide(
        &DecisionInput::new(revision, &enhanced, std::slice::from_ref(&candidate))
            .with_enhancement_guard(EnhancementGuard::from_fields([
                EnhancedField::Title,
                EnhancedField::Year,
            ])),
        &DecisionRules::v1(),
    );

    assert_eq!(decision.level, DecisionLevel::Confirmed);
    assert!(
        decision
            .reasons
            .contains(&DecisionReason::ExternalIdVerified)
    );
}

fn hint(kind: MediaKind, title: &str, year: Option<u16>) -> ParsedIdentityHint {
    ParsedIdentityHint {
        original: format!("{title}.mkv"),
        normalized_title: title.to_owned(),
        media_kind: kind,
        year,
        season: None,
        episodes: Vec::new(),
        air_date: None,
        external_ids: Vec::new(),
        version_tags: Vec::new(),
        parser_version: "test-parser-v1",
    }
}

fn movie(provider_id: i64, title: &str, year: Option<u16>) -> DecisionCandidate {
    DecisionCandidate {
        id: Uuid::now_v7(),
        identity: CandidateIdentity {
            provider_id,
            media_kind: ProviderMediaKind::Movie,
        },
        titles: vec![title.to_owned()],
        aliases: Vec::new(),
        year,
        locale: "en-US".to_owned(),
        original_title: None,
        release_dates: Vec::new(),
        episodes: Vec::new(),
        external_ids: Vec::new(),
        ranking_score: 0,
        provider_version: 1,
    }
}
