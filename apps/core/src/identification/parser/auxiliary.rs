use std::collections::BTreeSet;

use crate::identification::model::VersionTag;

pub(super) fn extract_version_tags(value: &str) -> (Vec<VersionTag>, BTreeSet<&'static str>) {
    let normalized = value.replace(['.', '_', '-'], " ").to_lowercase();
    let tokens = normalized.split_whitespace().collect::<Vec<_>>();
    let mut tags = BTreeSet::new();
    let mut removed = BTreeSet::new();
    for (index, token) in tokens.iter().enumerate() {
        match *token {
            "2160p" | "4k" | "uhd" => {
                tags.insert(VersionTag::Resolution2160p);
                removed.insert("2160p");
                removed.insert("4k");
                removed.insert("uhd");
            }
            "1080p" => {
                tags.insert(VersionTag::Resolution1080p);
                removed.insert("1080p");
            }
            "bluray" | "bdrip" => {
                tags.insert(VersionTag::BluRay);
                removed.insert("bluray");
                removed.insert("bdrip");
            }
            "webdl" => {
                tags.insert(VersionTag::WebDl);
                removed.insert("webdl");
            }
            "web" if tokens.get(index + 1) == Some(&"dl") => {
                tags.insert(VersionTag::WebDl);
                removed.insert("web");
                removed.insert("dl");
            }
            "remux" => {
                tags.insert(VersionTag::Remux);
                removed.insert("remux");
            }
            "x265" | "h265" | "hevc" => {
                tags.insert(VersionTag::Hevc);
                removed.insert("x265");
                removed.insert("h265");
                removed.insert("hevc");
            }
            "x264" | "h264" | "avc" => {
                tags.insert(VersionTag::H264);
                removed.insert("x264");
                removed.insert("h264");
                removed.insert("avc");
            }
            "hdr" | "hdr10" => {
                tags.insert(VersionTag::Hdr);
                removed.insert("hdr");
                removed.insert("hdr10");
            }
            "dv" | "dovi" => {
                tags.insert(VersionTag::DolbyVision);
                removed.insert("dv");
                removed.insert("dovi");
            }
            "extended" => {
                tags.insert(VersionTag::Extended);
                removed.insert("extended");
            }
            "repack" => {
                tags.insert(VersionTag::Repack);
                removed.insert("repack");
            }
            "proper" => {
                tags.insert(VersionTag::Proper);
                removed.insert("proper");
            }
            _ => {}
        }
    }
    if normalized.contains("director s cut") || normalized.contains("directors cut") {
        tags.insert(VersionTag::DirectorsCut);
        removed.insert("director");
        removed.insert("directors");
        removed.insert("s");
        removed.insert("cut");
    }
    (tags.into_iter().collect(), removed)
}
