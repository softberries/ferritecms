//! Engine-independent search tokenization (rulings R16, R17).
//!
//! Rust owns all text normalization: both the indexed text and the query go through
//! [`terms`], and the engines only ever see the resulting opaque tokens (SQLite FTS5 with
//! the `ascii` tokenizer, Postgres `tsvector`/`tsquery` literals with no parser or
//! locale). That is what makes both backends match exactly the same documents.

use std::collections::{HashMap, HashSet};

use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

/// Longest term kept, in characters (at most 256 bytes, far below Postgres' 2046-byte
/// lexeme limit); longer terms are truncated, on both the index and the query side.
pub const MAX_TERM_CHARS: usize = 64;
/// Most distinct terms a query uses; later ones are ignored.
pub const MAX_QUERY_TERMS: usize = 32;
/// Most distinct terms indexed per item (title and body together).
pub const MAX_INDEX_TERMS: usize = 4096;
/// Highest token position indexed per item (Postgres' tsvector position limit); tokens
/// after it are not indexed.
pub const MAX_POSITION: u16 = 16383;
/// Most positions recorded per term (Postgres keeps at most 256 per lexeme).
pub const MAX_POSITIONS_PER_TERM: usize = 256;
/// Budget for the total bytes of distinct indexed terms, keeping a Postgres tsvector well
/// under its 1 MB limit (4096 terms of 256 bytes would exceed it).
pub const MAX_INDEX_TERM_BYTES: usize = 512 * 1024;

/// The normalized token stream of `text`: NFKC, lowercased, split on everything that is
/// neither alphanumeric nor a combining mark (marks stay attached to their base), terms
/// without an alphabetic or numeric character dropped, each truncated to
/// [`MAX_TERM_CHARS`]. Repeats are kept.
fn terms(text: &str) -> Vec<String> {
    let normalized: String = text.nfkc().collect::<String>().to_lowercase();
    normalized
        .split(|c: char| !(c.is_alphanumeric() || is_combining_mark(c)))
        .filter(|t| t.chars().any(|c| c.is_alphabetic() || c.is_numeric()))
        .map(|t| t.chars().take(MAX_TERM_CHARS).collect())
        .collect()
}

/// Query terms: the distinct normalized terms of `q` in order, at most
/// [`MAX_QUERY_TERMS`]. Each is matched as a prefix and all must match. Empty means the
/// query cannot match anything (the stores then skip the database).
pub fn search_terms(q: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    terms(q)
        .into_iter()
        .filter(|t| seen.insert(t.clone()))
        .take(MAX_QUERY_TERMS)
        .collect()
}

/// One distinct indexed term of a field with its 1-based positions (shared numbering:
/// title tokens come first, then body tokens).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedTerm {
    pub term: String,
    pub positions: Vec<u16>,
}

/// What an item contributes to the index, per field, in first-occurrence order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexTerms {
    pub title: Vec<IndexedTerm>,
    pub body: Vec<IndexedTerm>,
}

impl IndexTerms {
    /// The field's kept tokens in position order, space-separated (the text SQLite's
    /// `ascii` tokenizer splits back into exactly these terms).
    pub fn joined(field: &[IndexedTerm]) -> String {
        let mut tokens: Vec<(u16, &str)> = field
            .iter()
            .flat_map(|t| t.positions.iter().map(move |&p| (p, t.term.as_str())))
            .collect();
        tokens.sort_unstable();
        tokens
            .into_iter()
            .map(|(_, t)| t)
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Splits an item's title and body into index terms with the limits above: at most
/// [`MAX_INDEX_TERMS`] distinct terms (and [`MAX_INDEX_TERM_BYTES`] of them), positions up
/// to [`MAX_POSITION`], at most [`MAX_POSITIONS_PER_TERM`] positions per term.
pub fn index_terms(title: &str, body: &str) -> IndexTerms {
    let mut out = IndexTerms::default();
    // Per field: term -> index in that field's list.
    let mut slots: [HashMap<String, usize>; 2] = [HashMap::new(), HashMap::new()];
    let mut positions_per_term: HashMap<String, usize> = HashMap::new();
    let mut distinct_bytes = 0;
    let mut position: u16 = 0;
    'fields: for (field, text) in [title, body].into_iter().enumerate() {
        for term in terms(text) {
            if position == MAX_POSITION {
                break 'fields;
            }
            position += 1;
            if !positions_per_term.contains_key(&term) {
                if positions_per_term.len() == MAX_INDEX_TERMS
                    || distinct_bytes + term.len() > MAX_INDEX_TERM_BYTES
                {
                    continue;
                }
                distinct_bytes += term.len();
            }
            let count = positions_per_term.entry(term.clone()).or_insert(0);
            if *count == MAX_POSITIONS_PER_TERM {
                continue;
            }
            *count += 1;
            let list = if field == 0 {
                &mut out.title
            } else {
                &mut out.body
            };
            let slot = *slots[field].entry(term.clone()).or_insert_with(|| {
                list.push(IndexedTerm {
                    term,
                    positions: Vec::new(),
                });
                list.len() - 1
            });
            list[slot].positions.push(position);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_terms_normalize_and_cap() {
        assert!(search_terms("").is_empty());
        assert!(search_terms(" \t\"*()-:&|!'\\<>^").is_empty());
        // A lone combining mark has no base character: dropped.
        assert!(search_terms("\u{301} \u{94d}").is_empty());
        assert_eq!(search_terms("  a OR  -x "), ["a", "or", "x"]);
        assert_eq!(search_terms("foo-bar \"baz\"* FOO"), ["foo", "bar", "baz"]);
        assert_eq!(search_terms("user@example.com"), ["user", "example", "com"]);
        // NFKC: fullwidth, superscripts and fractions fold to plain characters.
        assert_eq!(search_terms("ｆｏｏ x² ½"), ["foo", "x2", "1", "2"]);
        // NFD and NFC input give the same term; marks stay attached.
        assert_eq!(search_terms("U\u{308}ber"), search_terms("Über"));
        assert_eq!(search_terms("हिन्दी"), ["हिन्दी"]);
        assert_eq!(search_terms("İstanbul"), ["i\u{307}stanbul"]);
        let long = "a".repeat(3000);
        assert_eq!(search_terms(&long), ["a".repeat(MAX_TERM_CHARS)]);
        let many: Vec<String> = (0..40).map(|i| format!("t{i}")).collect();
        assert_eq!(search_terms(&many.join(" ")), many[..MAX_QUERY_TERMS]);
    }

    #[test]
    fn index_terms_positions_and_limits() {
        let t = index_terms("Hello world", "hello again, HELLO");
        assert_eq!(
            t.title,
            [
                IndexedTerm {
                    term: "hello".into(),
                    positions: vec![1]
                },
                IndexedTerm {
                    term: "world".into(),
                    positions: vec![2]
                },
            ]
        );
        assert_eq!(
            t.body,
            [
                IndexedTerm {
                    term: "hello".into(),
                    positions: vec![3, 5]
                },
                IndexedTerm {
                    term: "again".into(),
                    positions: vec![4]
                },
            ]
        );
        assert_eq!(IndexTerms::joined(&t.body), "hello again hello");

        let body: Vec<String> = (0..5000).map(|i| format!("w{i}")).collect();
        let t = index_terms("big", &body.join(" "));
        assert_eq!(t.title.len() + t.body.len(), MAX_INDEX_TERMS);
        assert_eq!(t.body.last().unwrap().term, "w4094");

        let t = index_terms("", &"same ".repeat(1000));
        assert_eq!(t.body[0].positions.len(), MAX_POSITIONS_PER_TERM);

        // 100 words x 200 = 20 000 tokens: indexing stops at position 16383.
        let text: String = (0..20_000).map(|i| format!("a{} ", i % 100)).collect();
        let t = index_terms("", &text);
        let max = t.body.iter().flat_map(|t| t.positions.iter()).max();
        assert_eq!(max, Some(&MAX_POSITION));
        let t = index_terms(
            "",
            &(0..20_000).map(|i| format!("u{i} ")).collect::<String>(),
        );
        assert_eq!(t.body.last().unwrap().positions, [MAX_INDEX_TERMS as u16]);
    }
}
