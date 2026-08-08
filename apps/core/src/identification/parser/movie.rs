use std::sync::LazyLock;

use regex::Regex;

static YEAR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[ ._\-\[(])(18[89]\d|19\d{2}|20\d{2}|21\d{2})(?:$|[ ._\-\])])")
        .expect("static year regex")
});

pub(super) fn take_year(value: &mut String) -> Option<u16> {
    let captures = YEAR.captures(value)?;
    let year = captures.get(1)?.as_str().parse().ok()?;
    let matched = captures.get(0)?;
    value.replace_range(matched.range(), " ");
    Some(year)
}
