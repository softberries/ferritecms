//! Backend-neutral search query terms (ruling R16).

/// Splits a user search query into the terms both backends match: maximal runs of
/// alphanumeric characters (`char::is_alphanumeric`). Everything else (whitespace,
/// punctuation, `"`, `-`, `*`, `(`, `:`, `&`, `|`, `!`, `'`, `\` ...) separates terms, so
/// no character with meaning to FTS5 or tsquery syntax can reach either engine, and
/// `foo-bar` means `foo` AND `bar`. Words such as `OR` or `NEAR` are ordinary terms.
/// Case is left to the engines (both lowercase). An empty result means "no terms".
pub fn search_terms(q: &str) -> Vec<String> {
    q.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::search_terms;

    #[test]
    fn terms_are_alphanumeric_runs() {
        assert!(search_terms("").is_empty());
        assert!(search_terms(" \t\"*()-:&|!'\\<>^").is_empty());
        assert_eq!(search_terms("  a OR  -x "), ["a", "OR", "x"]);
        assert_eq!(search_terms("foo-bar \"baz\"*"), ["foo", "bar", "baz"]);
        assert_eq!(
            search_terms("ünïcode日本 abc123"),
            ["ünïcode日本", "abc123"]
        );
        assert_eq!(
            search_terms("title:x NEAR(a b)"),
            ["title", "x", "NEAR", "a", "b"]
        );
        assert_eq!(search_terms("x_y it's"), ["x", "y", "it", "s"]);
    }
}
