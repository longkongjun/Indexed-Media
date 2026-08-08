use chrono::NaiveDate;
use mediaflow_core::connectors::model::{
    CandidateIdentity, EpisodeIdentity, ProviderError, ProviderMediaKind,
};
use mediaflow_core::identification::decision::{
    DecisionCandidate, DecisionInput, DecisionLevel, DecisionReason, DecisionRules, decide,
};
use mediaflow_core::identification::model::{ExternalIdHint, MediaKind, ParsedIdentityHint};
use uuid::Uuid;

#[test]
fn verified_tmdb_or_imdb_id_confirms_only_the_matching_basic_identity() {
    let revision = Uuid::now_v7();
    let mut local = hint(MediaKind::Movie, "流浪地球", Some(2019));
    local.external_ids = vec![external("tmdb", "535167")];
    let matching = movie(535_167, "流浪地球", Some(2019));

    let decision = decide(
        &DecisionInput::new(revision, &local, std::slice::from_ref(&matching)),
        &DecisionRules::v1(),
    );
    assert_eq!(decision.level, DecisionLevel::Confirmed);
    assert_eq!(decision.selected_candidate, Some(matching.id));
    assert!(
        decision
            .reasons
            .contains(&DecisionReason::ExternalIdVerified)
    );
    assert!(decision.graph.is_some());

    local.external_ids = vec![external("imdb", "tt7605074")];
    let mut imdb = matching;
    imdb.external_ids = vec![external("imdb", "tt7605074")];
    assert_eq!(
        decide(
            &DecisionInput::new(revision, &local, &[imdb]),
            &DecisionRules::v1(),
        )
        .level,
        DecisionLevel::Confirmed
    );
}

#[test]
fn external_id_with_type_or_basic_identity_conflict_is_ambiguous() {
    let revision = Uuid::now_v7();
    let mut local = hint(MediaKind::Movie, "流浪地球", Some(2019));
    local.external_ids = vec![external("tmdb", "535167")];
    let wrong_type = tv(535_167, "流浪地球", Some(2019), &[]);
    let decision = decide(
        &DecisionInput::new(revision, &local, &[wrong_type]),
        &DecisionRules::v1(),
    );
    assert_eq!(decision.level, DecisionLevel::Ambiguous);
    assert!(
        decision
            .reasons
            .contains(&DecisionReason::MediaTypeConflict)
    );

    let wrong_title = movie(535_167, "另一部电影", Some(2019));
    let decision = decide(
        &DecisionInput::new(revision, &local, &[wrong_title]),
        &DecisionRules::v1(),
    );
    assert_eq!(decision.level, DecisionLevel::Ambiguous);
    assert!(decision.reasons.contains(&DecisionReason::TitleConflict));
}

#[test]
fn unique_title_and_exact_or_explained_year_is_confirmed() {
    let revision = Uuid::now_v7();
    let local = hint(MediaKind::Movie, "千与千寻", Some(2001));
    let mut exact = movie(129, "Spirited Away", Some(2001));
    exact.aliases.push("千与千寻".to_owned());
    let decision = decide(
        &DecisionInput::new(revision, &local, &[exact]),
        &DecisionRules::v1(),
    );
    assert_eq!(decision.level, DecisionLevel::Confirmed);
    assert!(decision.reasons.contains(&DecisionReason::TitleMatched));
    assert!(decision.reasons.contains(&DecisionReason::YearMatched));

    let mut regional = movie(130, "千与千寻", Some(2002));
    regional.release_dates = vec![NaiveDate::from_ymd_opt(2001, 7, 20).unwrap()];
    let decision = decide(
        &DecisionInput::new(revision, &local, &[regional]),
        &DecisionRules::v1(),
    );
    assert_eq!(decision.level, DecisionLevel::Confirmed);
    assert!(
        decision
            .reasons
            .contains(&DecisionReason::RegionalReleaseDate)
    );
}

#[test]
fn missing_year_is_probable_and_equal_strong_candidates_are_ambiguous() {
    let revision = Uuid::now_v7();
    let no_year = hint(MediaKind::Movie, "沙丘", None);
    let one = movie(438_631, "沙丘", Some(2021));
    let decision = decide(
        &DecisionInput::new(revision, &no_year, std::slice::from_ref(&one)),
        &DecisionRules::v1(),
    );
    assert_eq!(decision.level, DecisionLevel::Probable);
    assert_eq!(decision.selected_candidate, Some(one.id));
    assert!(decision.reasons.contains(&DecisionReason::YearMissing));
    assert!(decision.graph.is_none());

    let local = hint(MediaKind::Movie, "沙丘", Some(2021));
    let other = movie(841, "沙丘", Some(2021));
    let decision = decide(
        &DecisionInput::new(revision, &local, &[one, other]),
        &DecisionRules::v1(),
    );
    assert_eq!(decision.level, DecisionLevel::Ambiguous);
    assert_eq!(decision.selected_candidate, None);
    assert!(
        decision
            .reasons
            .contains(&DecisionReason::MultipleStrongCandidates)
    );
}

#[test]
fn every_episode_must_exist_before_a_series_candidate_can_confirm() {
    let revision = Uuid::now_v7();
    let mut local = hint(MediaKind::Episode, "三体", Some(2023));
    local.season = Some(1);
    local.episodes = vec![1, 2, 3];
    let partial = tv(1_368_490, "三体", Some(2023), &[(1, 1), (1, 2)]);
    let decision = decide(
        &DecisionInput::new(revision, &local, &[partial]),
        &DecisionRules::v1(),
    );
    assert_eq!(decision.level, DecisionLevel::Ambiguous);
    assert!(decision.reasons.contains(&DecisionReason::EpisodeMissing));

    let complete = tv(1_368_490, "三体", Some(2023), &[(1, 1), (1, 2), (1, 3)]);
    let decision = decide(
        &DecisionInput::new(revision, &local, &[complete]),
        &DecisionRules::v1(),
    );
    assert_eq!(decision.level, DecisionLevel::Confirmed);
    assert_eq!(decision.graph.unwrap().episodes.len(), 3);
}

#[test]
fn provider_failure_is_blocked_and_popularity_never_overrides_identity_rules() {
    let revision = Uuid::now_v7();
    let local = hint(MediaKind::Movie, "低调作品", Some(2020));
    let retry_at_us = 123_000_000;
    let decision = decide(
        &DecisionInput::new(revision, &local, &[])
            .with_provider_error(ProviderError::RateLimited { retry_at_us }),
        &DecisionRules::v1(),
    );
    assert_eq!(decision.level, DecisionLevel::Blocked);
    assert_eq!(decision.retry_at_us, Some(retry_at_us));
    assert!(
        decision
            .reasons
            .contains(&DecisionReason::ProviderUnavailable)
    );

    let mut popular_wrong = movie(99, "热门但错误", Some(2020));
    popular_wrong.ranking_score = 10_000;
    let decision = decide(
        &DecisionInput::new(revision, &local, &[popular_wrong]),
        &DecisionRules::v1(),
    );
    assert_eq!(decision.level, DecisionLevel::Unidentified);
    assert_eq!(decision.selected_candidate, None);
}

#[test]
fn unsafe_nfo_evidence_prevents_an_otherwise_confirmed_match() {
    let revision = Uuid::now_v7();
    let local = hint(MediaKind::Movie, "安全边界", Some(2026));
    let candidate = movie(2026, "安全边界", Some(2026));
    let input = DecisionInput::new(revision, &local, std::slice::from_ref(&candidate))
        .with_local_conflicts(vec![DecisionReason::NfoUnsafe]);

    let decision = decide(&input, &DecisionRules::v1());
    assert_eq!(decision.level, DecisionLevel::Ambiguous);
    assert_eq!(decision.selected_candidate, None);
    assert_eq!(decision.reasons, vec![DecisionReason::NfoUnsafe]);
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

fn external(provider: &str, value: &str) -> ExternalIdHint {
    ExternalIdHint {
        provider: provider.to_owned(),
        value: value.to_owned(),
        is_default: true,
    }
}

fn movie(id: i64, title: &str, year: Option<u16>) -> DecisionCandidate {
    DecisionCandidate {
        id: Uuid::now_v7(),
        identity: CandidateIdentity {
            provider_id: id,
            media_kind: ProviderMediaKind::Movie,
        },
        titles: vec![title.to_owned()],
        aliases: Vec::new(),
        year,
        locale: "zh-CN".to_owned(),
        original_title: None,
        release_dates: Vec::new(),
        episodes: Vec::new(),
        external_ids: Vec::new(),
        ranking_score: 0,
        provider_version: 1,
    }
}

fn tv(id: i64, title: &str, year: Option<u16>, episodes: &[(u16, u16)]) -> DecisionCandidate {
    let mut candidate = movie(id, title, year);
    candidate.identity.media_kind = ProviderMediaKind::Tv;
    candidate.episodes = episodes
        .iter()
        .map(|&(season, episode)| EpisodeIdentity { season, episode })
        .collect();
    candidate
}
