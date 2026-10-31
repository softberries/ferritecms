//! Every fixed `id` the starter themes write is one of core's reserved fragment ids
//! (M4-3b P54, 06 §4), so a heading id from a body can never take it: the block renderer
//! seeds its counter with them and the plugin sanitiser drops them from headings.
//!
//! Read from the template sources: literal `id="…"` values, `form_id` values a template
//! passes to a partial or a macro, and the ids that partial or macro derives from it
//! (`id="{{ form_id }}"`, `id="{{ form_id }}-author"`). Ids built from data
//! (`comment-{{ c.id }}`, `"reply-" ~ c.id`) are not fixed and not checked.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use ferrite_domain::html::RESERVED_FRAGMENT_IDS;

const THEMES: [&str; 3] = ["blog", "business", "docs"];

fn templates(theme: &str) -> BTreeMap<String, String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../themes")
        .join(theme)
        .join("templates");
    let mut out = BTreeMap::new();
    let mut stack: Vec<PathBuf> = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).unwrap() {
            let path = e.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|x| x == "html") {
                let rel = path
                    .strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(rel, std::fs::read_to_string(&path).unwrap());
            }
        }
    }
    out
}

/// The double-quoted values that follow each `needle` in `text`.
fn after<'a>(text: &'a str, needle: &str) -> Vec<&'a str> {
    text.match_indices(needle)
        .map(|(i, m)| {
            let rest = &text[i + m.len()..];
            &rest[..rest.find('"').unwrap()]
        })
        .collect()
}

/// What a template derives from `form_id`: whether it uses it bare as an id, and the
/// suffixes of `id="{{ form_id }}-<suffix>"`.
fn derived(text: &str) -> (bool, Vec<String>) {
    let ids = after(text, " id=\"{{ form_id }}");
    let bare = ids.contains(&"");
    let suffixes = ids
        .iter()
        .filter_map(|s| s.strip_prefix('-'))
        .map(str::to_owned)
        .collect();
    (bare, suffixes)
}

fn with_derived(out: &mut BTreeSet<String>, form_id: &str, (bare, suffixes): &(bool, Vec<String>)) {
    if *bare {
        out.insert(form_id.to_owned());
    }
    for s in suffixes {
        out.insert(format!("{form_id}-{s}"));
    }
}

/// The fixed ids `theme`'s templates can write.
fn fixed_ids(theme: &str) -> BTreeSet<String> {
    let files = templates(theme);
    let mut out = BTreeSet::new();
    for text in files.values() {
        for id in after(text, " id=\"") {
            if !id.contains('{') {
                out.insert(id.to_owned());
            }
        }
        // `{% with form_id = "x" … %}{% include "partials/p.html" %}`.
        for (i, _) in text.match_indices("form_id = \"") {
            let rest = &text[i + "form_id = \"".len()..];
            let form_id = &rest[..rest.find('"').unwrap()];
            let line = rest.lines().next().unwrap();
            let partial = after(line, "include \"");
            assert_eq!(partial.len(), 1, "{theme}: {line}");
            with_derived(&mut out, form_id, &derived(&files[partial[0]]));
        }
        // `{{ comment_form(thread, none, "x") }}`: the macro is defined in this file.
        for (i, _) in text.match_indices("{{ comment_form(") {
            let args = &text[i..];
            let args = &args[..args.find(')').unwrap()];
            let last = args.rsplit(',').next().unwrap().trim();
            if let Some(form_id) = last.strip_prefix('"').and_then(|l| l.strip_suffix('"')) {
                with_derived(&mut out, form_id, &derived(text));
            }
        }
    }
    out
}

#[test]
fn the_starter_themes_fixed_ids_are_reserved() {
    let mut all = BTreeSet::new();
    let mut missing = Vec::new();
    for theme in THEMES {
        for id in fixed_ids(theme) {
            if !RESERVED_FRAGMENT_IDS.contains(&id.as_str()) {
                missing.push(format!("{theme}: {id}"));
            }
            all.insert(id);
        }
    }
    assert!(
        missing.is_empty(),
        "not in RESERVED_FRAGMENT_IDS: {missing:?}"
    );
    // The scan found each kind of id (it is not vacuous).
    for id in [
        "content",
        "comment-pending",
        "comment-form-author",
        "search-main",
        "cf-name",
        "docs-search-page",
        "search",
    ] {
        assert!(all.contains(id), "{id} not found: {all:?}");
    }
}
