use mediaflow_core::identification::model::{MediaKind, VersionTag};
use mediaflow_core::identification::parser::FilenameParser;

#[test]
fn unicode_movies_preserve_original_text_and_strip_only_explicit_version_evidence() {
    let parser = FilenameParser::default();
    let parsed = parser
        .parse("电影/流浪地球 (2019)/流浪地球.2019.2160p.BluRay.x265-GROUP.mkv")
        .unwrap();

    assert_eq!(parsed.media_kind, MediaKind::Movie);
    assert_eq!(parsed.normalized_title, "流浪地球");
    assert_eq!(parsed.year, Some(2019));
    assert!(parsed.version_tags.contains(&VersionTag::Resolution2160p));
    assert!(parsed.version_tags.contains(&VersionTag::BluRay));
    assert!(parsed.version_tags.contains(&VersionTag::Hevc));
    assert!(parsed.original.contains("流浪地球"));
}

#[test]
fn episode_ranges_date_episodes_non_latin_titles_and_nfkc_are_deterministic() {
    let parser = FilenameParser::default();
    let range = parser
        .parse("剧集/三体/Season 01/三体.S01E01-E03.2023.1080p.WEB-DL.mkv")
        .unwrap();
    assert_eq!(range.media_kind, MediaKind::Episode);
    assert_eq!(range.normalized_title, "三体");
    assert_eq!(range.year, Some(2023));
    assert_eq!(range.season, Some(1));
    assert_eq!(range.episodes, vec![1, 2, 3]);

    let sparse = parser.parse("Show.S01E01E02E04.mkv").unwrap();
    assert_eq!(sparse.season, Some(1));
    assert_eq!(sparse.episodes, vec![1, 2, 4]);

    let japanese = parser.parse("アニメ/進撃の巨人.S02E03.mkv").unwrap();
    assert_eq!(japanese.normalized_title, "進撃の巨人");
    assert_eq!(japanese.season, Some(2));
    assert_eq!(japanese.episodes, vec![3]);

    let dated = parser
        .parse("Daily.Show.2026.07.23.1080p.WEB-DL.mkv")
        .unwrap();
    assert_eq!(dated.media_kind, MediaKind::Episode);
    assert_eq!(dated.normalized_title, "daily show");
    assert_eq!(dated.air_date.as_deref(), Some("2026-07-23"));

    let full_width = parser.parse("Ｍｏｖｉｅ．２０２０.mkv").unwrap();
    assert_eq!(full_width.normalized_title, "movie");
    assert_eq!(full_width.year, Some(2020));
}

#[test]
fn explicit_provider_ids_are_bounded_evidence_and_malformed_names_fail_closed() {
    let parser = FilenameParser::default();
    let parsed = parser
        .parse("Dune (2021) [tmdb-438631] [imdb-tt1160419].mkv")
        .unwrap();
    assert_eq!(parsed.normalized_title, "dune");
    assert_eq!(parsed.external_ids.len(), 2);
    assert_eq!(parsed.external_ids[0].provider, "tmdb");
    assert_eq!(parsed.external_ids[0].value, "438631");
    assert_eq!(parsed.external_ids[1].provider, "imdb");
    assert_eq!(parsed.external_ids[1].value, "tt1160419");

    assert!(parser.parse("").is_err());
    assert!(parser.parse(&format!("{}.mkv", "x".repeat(4097))).is_err());
    assert!(parser.parse("../escape/movie.mkv").is_err());
    assert!(parser.parse("movie.S01E03-E01.mkv").is_err());
}
