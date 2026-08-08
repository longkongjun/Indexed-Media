use mediaflow_core::identification::model::{CandidateIdentityGraph, CandidateWork, MediaKind};
use mediaflow_core::identification::parser::FilenameParser;

#[test]
fn one_multiepisode_file_revision_maps_to_one_version_and_three_episode_nodes() {
    let revision_id = uuid::Uuid::now_v7();
    let hint = FilenameParser::default()
        .parse("Shows/三体.S01E01-E03.2160p.mkv")
        .unwrap();
    let graph = CandidateIdentityGraph::from_hint(revision_id, &hint).unwrap();

    assert_eq!(graph.file_revision_id, revision_id);
    assert_eq!(graph.media_kind, MediaKind::Episode);
    assert!(matches!(graph.work, CandidateWork::Series { ref title } if title == "三体"));
    assert_eq!(graph.seasons.len(), 1);
    assert_eq!(graph.seasons[0].season, 1);
    assert_eq!(graph.media_versions.len(), 1);
    assert_eq!(graph.episodes.len(), 3);
    assert_eq!(
        graph
            .episodes
            .iter()
            .map(|episode| (episode.season, episode.episode))
            .collect::<Vec<_>>(),
        vec![(1, 1), (1, 2), (1, 3)]
    );
}

#[test]
fn movie_hint_builds_candidate_types_without_creating_a_catalog_identity() {
    let revision_id = uuid::Uuid::now_v7();
    let hint = FilenameParser::default()
        .parse("Movies/沙丘.Dune.2021.4K.mkv")
        .unwrap();
    let graph = CandidateIdentityGraph::from_hint(revision_id, &hint).unwrap();

    assert!(matches!(graph.work, CandidateWork::Movie { .. }));
    assert!(graph.seasons.is_empty());
    assert!(graph.episodes.is_empty());
    assert_eq!(graph.media_versions.len(), 1);
}
