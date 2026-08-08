use mediaflow_core::discovery::auxiliary::{AuxiliaryReason, classify_auxiliary};

#[test]
fn explicit_sample_trailer_and_extras_markers_are_classified_with_reasons() {
    for (path, expected) in [
        ("Movie.2020.sample.mkv", AuxiliaryReason::Sample),
        ("Movie 2020-trailer.mp4", AuxiliaryReason::Trailer),
        ("Movie/Extras/interview.mkv", AuxiliaryReason::Extra),
        ("电影/花絮/制作特辑.mkv", AuxiliaryReason::Extra),
    ] {
        assert_eq!(classify_auxiliary(path), Some(expected), "{path}");
    }
}

#[test]
fn size_duration_and_titles_containing_marker_words_never_classify_by_themselves() {
    for path in [
        "Sample (2019).mkv",
        "Trailer Park Boys S01E01.mkv",
        "Extras (2025).mkv",
        "电影/正片.mkv",
        "损坏�名称.mkv",
    ] {
        assert_eq!(classify_auxiliary(path), None, "{path}");
    }
}
