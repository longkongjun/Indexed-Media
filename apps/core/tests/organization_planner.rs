use std::collections::BTreeSet;

use mediaflow_core::discovery::model::{RelativePath, RootId};
use mediaflow_core::organization::model::{
    ConfirmedNfoMedia, ConfirmedNfoMetadata, ConfirmedProviderId, NfoProvider,
    OrganizationNamingPattern, OrganizationNfoPolicy, OrganizationOperation, OrganizationRuleInput,
    OrganizationTarget, OrganizationTargetKind,
};
use mediaflow_core::organization::planner::{
    OrganizationLocation, OrganizationPlanner, PlanAuthorization, PlanningDecision,
    PlanningIdentity, PlanningInput, PlanningRiskCode,
};
use uuid::Uuid;

fn target(kind: OrganizationTargetKind, automatic: bool) -> OrganizationTarget {
    let naming_pattern = match kind {
        OrganizationTargetKind::Movie => OrganizationNamingPattern::Movie,
        OrganizationTargetKind::Series => OrganizationNamingPattern::Series,
        OrganizationTargetKind::GenericVideo => OrganizationNamingPattern::GenericNumbered,
    };
    OrganizationTarget {
        id: Uuid::now_v7(),
        kind,
        display_name: "Library".to_owned(),
        root_id: RootId::parse("media").unwrap(),
        relative_path: RelativePath::parse("Library").unwrap(),
        operation: OrganizationOperation::Copy,
        naming_pattern,
        nfo_policy: OrganizationNfoPolicy::GenerateMissing,
        automatic,
        enabled: true,
        rules: vec![OrganizationRuleInput {
            media_kind: kind,
            inbox_directory_id: Some(Uuid::from_u128(1)),
            explicit_tag: Some("trusted".to_owned()),
            enabled: true,
        }],
        config_version: 7,
        updated_at_us: 100,
    }
}

fn input(identity: PlanningIdentity, target: OrganizationTarget) -> PlanningInput {
    PlanningInput {
        task_id: Uuid::now_v7(),
        file_revision_id: Uuid::now_v7(),
        selected_identity_id: Some(Uuid::now_v7()),
        source: OrganizationLocation {
            root_id: RootId::parse("incoming").unwrap(),
            relative_path: RelativePath::parse("ready/source.mkv").unwrap(),
        },
        source_inbox_id: Uuid::from_u128(1),
        source_writable: true,
        source_unchanged: true,
        destination_exists: false,
        same_filesystem: true,
        explicit_tags: BTreeSet::from(["trusted".to_owned()]),
        one_time_authorized: false,
        identity,
        nfo_metadata: ConfirmedNfoMetadata::default(),
        target,
    }
}

#[test]
fn confirmed_movie_uses_deterministic_naming_and_auto_rule_provenance() {
    let target = target(OrganizationTargetKind::Movie, true);
    let target_id = target.id;
    let mut planning_input = input(
        PlanningIdentity::Movie {
            title: "流浪地球".to_owned(),
            year: Some(2019),
            version_label: Some("2160p".to_owned()),
        },
        target,
    );
    planning_input.nfo_metadata = ConfirmedNfoMetadata {
        original_title: Some("The Wandering Earth".to_owned()),
        year: Some(2019),
        plot: None,
        provider_id: Some(ConfirmedProviderId {
            provider: NfoProvider::Tmdb,
            value: "535167".to_owned(),
        }),
    };
    let draft = OrganizationPlanner::plan(&planning_input).unwrap();

    assert_eq!(draft.authorization, PlanAuthorization::Automatic);
    assert!(draft.risk_codes.is_empty());
    assert_eq!(
        draft.destination.relative_path.as_str(),
        "Library/流浪地球 (2019)/流浪地球 (2019) - 2160p.mkv"
    );
    assert_eq!(draft.operations.len(), 2, "file plus ensure-missing NFO");
    let nfo = draft.nfo_input.as_ref().expect("confirmed NFO input");
    assert_eq!(nfo.file_name, "流浪地球 (2019) - 2160p.nfo");
    assert_eq!(
        nfo.media,
        ConfirmedNfoMedia::Movie {
            title: "流浪地球".to_owned(),
            original_title: Some("The Wandering Earth".to_owned()),
            year: Some(2019),
            plot: None,
        }
    );
    assert_eq!(nfo.provider_id, planning_input.nfo_metadata.provider_id);
    assert!(draft.provenance.iter().any(|value| {
        value.field == "rule" && value.source_id == Some(target_id) && value.source_version == 7
    }));
}

#[test]
fn confirmed_series_builds_season_episode_and_version_hierarchy() {
    let draft = OrganizationPlanner::plan(&input(
        PlanningIdentity::SeriesEpisode {
            series_title: "The Expanse".to_owned(),
            season: 2,
            episodes: vec![1, 2],
            version_label: Some("WEB-DL".to_owned()),
        },
        target(OrganizationTargetKind::Series, false),
    ))
    .unwrap();

    assert_eq!(draft.authorization, PlanAuthorization::Paused);
    assert_eq!(
        draft.destination.relative_path.as_str(),
        "Library/The Expanse/Season 02/The Expanse - S02E01-E02 - WEB-DL.mkv"
    );
}

#[test]
fn generic_video_requires_a_unique_group_and_sequence() {
    let generic_target = target(OrganizationTargetKind::GenericVideo, true);
    let ambiguous = OrganizationPlanner::plan(&input(
        PlanningIdentity::GenericVideo {
            title: "Kotlin Coroutines".to_owned(),
            group: None,
            sequence: None,
        },
        generic_target.clone(),
    ))
    .unwrap_err();
    assert_eq!(ambiguous, PlanningDecision::GenericGroupingAmbiguous);

    let draft = OrganizationPlanner::plan(&input(
        PlanningIdentity::GenericVideo {
            title: "Kotlin Coroutines".to_owned(),
            group: Some("Kotlin 深入".to_owned()),
            sequence: Some(3),
        },
        generic_target,
    ))
    .unwrap();
    assert_eq!(
        draft.destination.relative_path.as_str(),
        "Library/Kotlin 深入/Kotlin 深入 - 003 - Kotlin Coroutines.mkv"
    );
}

#[test]
fn unsafe_or_reserved_names_never_escape_the_target_root() {
    let sanitized = OrganizationPlanner::plan(&input(
        PlanningIdentity::Movie {
            title: "../../CON/Alien".to_owned(),
            year: Some(1979),
            version_label: None,
        },
        target(OrganizationTargetKind::Movie, false),
    ))
    .unwrap();
    let path = sanitized.destination.relative_path.as_str();
    assert!(path.starts_with("Library/"));
    assert!(!path.contains("../"));
    assert!(!path.starts_with('/'));

    let rejected = OrganizationPlanner::plan(&input(
        PlanningIdentity::Movie {
            title: "...".to_owned(),
            year: None,
            version_label: None,
        },
        target(OrganizationTargetKind::Movie, false),
    ))
    .unwrap_err();
    assert_eq!(rejected, PlanningDecision::PathOutsideRoot);
}

#[test]
fn missing_rule_pauses_but_one_time_authorization_can_cover_only_that_gap() {
    let mut without_rule = input(
        PlanningIdentity::Movie {
            title: "Arrival".to_owned(),
            year: Some(2016),
            version_label: None,
        },
        target(OrganizationTargetKind::Movie, true),
    );
    without_rule.explicit_tags.clear();
    let paused = OrganizationPlanner::plan(&without_rule).unwrap();
    assert_eq!(paused.authorization, PlanAuthorization::Paused);
    assert_eq!(paused.risk_codes, vec![PlanningRiskCode::RuleNotMatched]);

    without_rule.one_time_authorized = true;
    let authorized = OrganizationPlanner::plan(&without_rule).unwrap();
    assert_eq!(authorized.authorization, PlanAuthorization::OneTime);
    assert!(authorized.risk_codes.is_empty());
}

#[test]
fn one_time_authorization_cannot_override_target_source_or_hardlink_invariants() {
    let mut unsafe_input = input(
        PlanningIdentity::Movie {
            title: "Dune".to_owned(),
            year: Some(2021),
            version_label: None,
        },
        target(OrganizationTargetKind::Movie, true),
    );
    unsafe_input.target.operation = OrganizationOperation::Hardlink;
    unsafe_input.destination_exists = true;
    unsafe_input.source_unchanged = false;
    unsafe_input.same_filesystem = false;
    unsafe_input.one_time_authorized = true;

    let draft = OrganizationPlanner::plan(&unsafe_input).unwrap();
    assert_eq!(draft.authorization, PlanAuthorization::Paused);
    assert_eq!(
        draft.risk_codes,
        vec![
            PlanningRiskCode::SourceChanged,
            PlanningRiskCode::TargetExists,
            PlanningRiskCode::HardlinkCrossDevice,
        ]
    );
}

#[test]
fn preserve_only_profile_does_not_capture_generation_input() {
    let mut preserve = target(OrganizationTargetKind::Movie, true);
    preserve.nfo_policy = OrganizationNfoPolicy::PreserveOnly;
    let draft = OrganizationPlanner::plan(&input(
        PlanningIdentity::Movie {
            title: "Arrival".to_owned(),
            year: Some(2016),
            version_label: None,
        },
        preserve,
    ))
    .unwrap();

    assert!(draft.nfo_input.is_none());
    assert_eq!(draft.operations.len(), 1);
}
