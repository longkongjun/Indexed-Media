use std::sync::LazyLock;

use chrono::NaiveDate;
use regex::Regex;

static COMPACT_MULTI_EPISODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[ ._\-\[])s(\d{1,2})e(\d{1,3})((?:e\d{1,3}){1,99})(?:$|[ ._\-\]])")
        .expect("static compact multi-episode regex")
});
static COMPACT_EPISODE_PART: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)e(\d{1,3})").expect("static compact episode part regex"));
static SEASON_EPISODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[ ._\-\[])s(\d{1,2})e(\d{1,3})(?:\s*-\s*e?(\d{1,3}))?(?:$|[ ._\-\]])")
        .expect("static season episode regex")
});
static X_EPISODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[ ._\-\[])(\d{1,2})x(\d{1,3})(?:$|[ ._\-\]])")
        .expect("static x episode regex")
});
static DATE_EPISODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:^|[ ._\-\[])(20\d{2})[._\-](0[1-9]|1[0-2])[._\-](0[1-9]|[12]\d|3[01])(?:$|[ ._\-\]])",
    )
    .expect("static date episode regex")
});

pub(super) struct EpisodeFields {
    pub season: Option<u16>,
    pub episodes: Vec<u16>,
    pub air_date: Option<String>,
}

pub(super) fn take_episode(value: &mut String) -> Result<Option<EpisodeFields>, ()> {
    if let Some(captures) = COMPACT_MULTI_EPISODE.captures(value) {
        let season = parse_capture(&captures, 1)?;
        let first = parse_capture(&captures, 2)?;
        let tail = captures.get(3).ok_or(())?.as_str();
        let mut episodes = vec![first];
        for capture in COMPACT_EPISODE_PART.captures_iter(tail) {
            episodes.push(parse_capture(&capture, 1)?);
        }
        if episodes.len() > 100
            || episodes.contains(&0)
            || episodes.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err(());
        }
        let matched = captures.get(0).ok_or(())?;
        value.replace_range(matched.range(), " ");
        return Ok(Some(EpisodeFields {
            season: Some(season),
            episodes,
            air_date: None,
        }));
    }
    if let Some(captures) = SEASON_EPISODE.captures(value) {
        let season = parse_capture(&captures, 1)?;
        let start = parse_capture(&captures, 2)?;
        let end = captures
            .get(3)
            .map(|value| value.as_str().parse::<u16>().map_err(|_| ()))
            .transpose()?
            .unwrap_or(start);
        if start == 0 || end < start || end.saturating_sub(start) >= 100 {
            return Err(());
        }
        let matched = captures.get(0).ok_or(())?;
        value.replace_range(matched.range(), " ");
        return Ok(Some(EpisodeFields {
            season: Some(season),
            episodes: (start..=end).collect(),
            air_date: None,
        }));
    }
    if let Some(captures) = X_EPISODE.captures(value) {
        let season = parse_capture(&captures, 1)?;
        let episode = parse_capture(&captures, 2)?;
        if episode == 0 {
            return Err(());
        }
        let matched = captures.get(0).ok_or(())?;
        value.replace_range(matched.range(), " ");
        return Ok(Some(EpisodeFields {
            season: Some(season),
            episodes: vec![episode],
            air_date: None,
        }));
    }
    if let Some(captures) = DATE_EPISODE.captures(value) {
        let year = i32::from(parse_capture(&captures, 1)?);
        let month = u32::from(parse_capture(&captures, 2)?);
        let day = u32::from(parse_capture(&captures, 3)?);
        let date = NaiveDate::from_ymd_opt(year, month, day).ok_or(())?;
        let matched = captures.get(0).ok_or(())?;
        value.replace_range(matched.range(), " ");
        return Ok(Some(EpisodeFields {
            season: None,
            episodes: Vec::new(),
            air_date: Some(date.format("%Y-%m-%d").to_string()),
        }));
    }
    Ok(None)
}

fn parse_capture(captures: &regex::Captures<'_>, index: usize) -> Result<u16, ()> {
    captures
        .get(index)
        .ok_or(())?
        .as_str()
        .parse()
        .map_err(|_| ())
}
