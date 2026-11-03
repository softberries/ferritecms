//! Vague link phrases per language (design 16 §4.2, D3). Data: adding a language is a
//! new entry here.

const EN: &[&str] = &[
    "click here",
    "here",
    "read more",
    "more",
    "learn more",
    "this",
    "link",
    "this link",
    "continue",
    "details",
    "go",
];
const DE: &[&str] = &[
    "hier",
    "hier klicken",
    "klicken sie hier",
    "mehr",
    "weiterlesen",
    "mehr erfahren",
];
const FR: &[&str] = &[
    "ici",
    "cliquez ici",
    "en savoir plus",
    "lire la suite",
    "plus",
];
const ES: &[&str] = &[
    "aquí",
    "haz clic aquí",
    "leer más",
    "más",
    "más información",
];
const PL: &[&str] = &["tutaj", "kliknij tutaj", "więcej", "czytaj więcej"];

/// The list of a primary language subtag (lowercase), besides English.
fn list_for(language: &str) -> &'static [&'static str] {
    match language {
        "de" => DE,
        "fr" => FR,
        "es" => ES,
        "pl" => PL,
        _ => &[],
    }
}

/// The phrase as compared: lowercased, trailing `.`, `…`, `:`, `»`, `→` (and the
/// whitespace between them) removed.
pub(super) fn normalize(text: &str) -> String {
    let lower = text.to_lowercase();
    lower
        .trim_end_matches(|c: char| matches!(c, '.' | '…' | ':' | '»' | '→') || c.is_whitespace())
        .trim()
        .to_owned()
}

/// Whether `text` (a link's joined, trimmed text) is a vague phrase of `locale`'s
/// language or of English.
pub fn is_vague(text: &str, locale: &str) -> bool {
    let phrase = normalize(text);
    if phrase.is_empty() {
        return false;
    }
    let language = locale
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    EN.contains(&phrase.as_str()) || list_for(&language).contains(&phrase.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_locale() {
        assert!(is_vague("Read more…", "en"));
        assert!(is_vague("Click here →", "en-GB"));
        assert!(is_vague("Weiterlesen", "de-AT"));
        assert!(
            is_vague("here", "de-AT"),
            "English applies to every language"
        );
        assert!(!is_vague("weiterlesen", "ja"), "ja uses English only");
        assert!(is_vague("more", "ja"));
        assert!(is_vague("Więcej", "pl"));
        assert!(is_vague("MÁS", "es"));
        assert!(!is_vague("Read more about lakes", "en"));
        assert!(!is_vague("", "en"));
        assert!(!is_vague("...", "en"));
    }
}
