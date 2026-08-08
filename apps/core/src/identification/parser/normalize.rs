use unicode_normalization::UnicodeNormalization as _;

pub(super) fn nfkc_lower(value: &str) -> String {
    value.nfkc().flat_map(char::to_lowercase).collect()
}

pub(super) fn fold_separators(value: &str) -> String {
    let mut folded = String::with_capacity(value.len());
    let mut pending_space = false;
    for character in value.chars() {
        if character.is_whitespace()
            || matches!(
                character,
                '.' | '_' | '-' | '+' | '(' | ')' | '[' | ']' | '{' | '}'
            )
        {
            pending_space = !folded.is_empty();
        } else {
            if pending_space {
                folded.push(' ');
                pending_space = false;
            }
            folded.push(character);
        }
    }
    folded.trim().to_owned()
}
