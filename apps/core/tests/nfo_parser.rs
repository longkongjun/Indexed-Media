use mediaflow_core::identification::nfo::{NfoKind, NfoParseError, NfoParser};
use sha2::{Digest, Sha256};

const MOVIE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packages/test-fixtures/src/m3/nfo/valid/movie.nfo"
));
const EPISODE: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packages/test-fixtures/src/m3/nfo/valid/episode.nfo"
));
const TV_SHOW: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packages/test-fixtures/src/m3/nfo/valid/tvshow.nfo"
));
const STACKED_EPISODES: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packages/test-fixtures/src/m3/nfo/valid/episode-stacked-v21.nfo"
));

#[test]
fn parses_bounded_kodi_movie_episode_and_tv_show_fields() {
    let parser = NfoParser::default();
    let parsed_movie = parser.parse(MOVIE).unwrap();
    let parsed_episode = parser.parse(EPISODE).unwrap();
    let parsed_tv_show = parser.parse(TV_SHOW).unwrap();
    let movie = &parsed_movie.documents[0];
    let episode = &parsed_episode.documents[0];
    let tv_show = &parsed_tv_show.documents[0];

    assert_eq!(movie.kind, NfoKind::Movie);
    assert_eq!(
        parsed_movie.document_hash,
        <[u8; 32]>::from(Sha256::digest(MOVIE))
    );
    assert_eq!(movie.title.as_deref(), Some("Blade Runner 2049"));
    assert_eq!(movie.original_title.as_deref(), Some("Blade Runner 2049"));
    assert_eq!(movie.year, Some(2017));
    assert_eq!(movie.premiered.as_deref(), Some("2017-10-06"));
    assert_eq!(movie.external_ids.len(), 2);
    assert_eq!(movie.external_ids[0].provider, "tmdb");
    assert_eq!(movie.external_ids[0].value, "335984");
    assert!(movie.external_ids[0].is_default);
    assert_eq!(movie.external_ids[1].provider, "imdb");

    assert_eq!(episode.kind, NfoKind::Episode);
    assert_eq!(episode.show_title.as_deref(), Some("The IT Crowd"));
    assert_eq!(episode.season, Some(2));
    assert_eq!(episode.episode, Some(1));
    assert_eq!(episode.aired.as_deref(), Some("2007-08-24"));

    assert_eq!(tv_show.kind, NfoKind::TvShow);
    assert_eq!(tv_show.title.as_deref(), Some("The IT Crowd"));
    assert_eq!(tv_show.year, Some(2006));
}

#[test]
fn keeps_multiple_default_ids_as_evidence_instead_of_resolving_the_conflict() {
    let parsed = NfoParser::default()
        .parse(
            br#"<movie>
                <title>Conflict</title>
                <uniqueid type="tmdb" default="true">1</uniqueid>
                <uniqueid type="imdb" default="true">tt2</uniqueid>
            </movie>"#,
        )
        .unwrap();
    let document = &parsed.documents[0];

    assert_eq!(document.external_ids.len(), 2);
    assert!(document.external_ids.iter().all(|id| id.is_default));
}

#[test]
fn parses_kodi_v21_stacked_episode_documents_in_order() {
    let parsed = NfoParser::default().parse(STACKED_EPISODES).unwrap();

    assert_eq!(parsed.root_kind, NfoKind::Episode);
    assert_eq!(parsed.documents.len(), 2);
    assert_eq!(parsed.documents[0].episode, Some(1));
    assert_eq!(parsed.documents[1].episode, Some(2));
    assert_eq!(parsed.documents[1].title.as_deref(), Some("Second"));
}

#[test]
fn rejects_unsupported_roots_and_invalid_utf8() {
    let parser = NfoParser::default();

    assert_eq!(
        parser.parse(b"<musicvideo><title>x</title></musicvideo>"),
        Err(NfoParseError::UnsupportedRoot)
    );
    assert_eq!(
        parser.parse(b"<movie><title>\xff</title></movie>"),
        Err(NfoParseError::InvalidUtf8)
    );
}
