//! Template hierarchy (06 §2): which template file renders a route. Pure.

/// What is being rendered, as far as template selection cares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateKey {
    /// A single item of a non-page type.
    Single { kind: String, slug: String },
    /// A page (`id` is the content id as a string).
    Page { slug: String, id: String },
    /// A content type archive.
    Archive { kind: String },
    /// A term archive.
    Term { tax: String, slug: String },
    /// An author archive.
    Author { login: String },
    /// A date archive.
    Date,
    /// Search results.
    Search,
    /// The site root showing a static page (`reading.front_page`).
    FrontPage { slug: String, id: String },
    /// The site root listing posts.
    FrontPosts,
    /// 404.
    NotFound,
}

/// The theme's 500 template, if it has one; `None` means the caller renders the
/// built-in error page (06 §7). Unlike every route, there is no `index.html`
/// fallback: the index is a normal page and may need data an error page lacks.
pub fn error_template(has: impl Fn(&str) -> bool) -> Option<&'static str> {
    has(ERROR).then_some(ERROR)
}

const ERROR: &str = "500.html";

/// The first existing candidate for `key` (06 §2 table), `index.html` when none does.
///
/// Term archives of `category` and `tag` (or `post_tag`) also try the WordPress names
/// `category-{slug}.html`/`category.html` (`tag-…` for tags); each alias is tried just
/// before its `taxonomy-…` equivalent, so a slug-specific template always beats a
/// taxonomy-wide one.
pub fn resolve_template(key: &TemplateKey, has: impl Fn(&str) -> bool) -> String {
    candidates(key)
        .into_iter()
        .find(|c| has(c))
        .unwrap_or_else(|| INDEX.to_owned())
}

const INDEX: &str = "index.html";

fn candidates(key: &TemplateKey) -> Vec<String> {
    let mut c: Vec<String> = match key {
        TemplateKey::Single { kind, slug } => vec![
            format!("single-{kind}-{slug}.html"),
            format!("single-{kind}.html"),
            "single.html".into(),
        ],
        TemplateKey::Page { slug, id } => page_row(slug, id),
        TemplateKey::Archive { kind } => {
            vec![format!("archive-{kind}.html"), "archive.html".into()]
        }
        TemplateKey::Term { tax, slug } => {
            let alias = match tax.as_str() {
                "category" => Some("category"),
                "tag" | "post_tag" => Some("tag"),
                _ => None,
            };
            let mut v = Vec::new();
            if let Some(a) = alias {
                v.push(format!("{a}-{slug}.html"));
            }
            v.push(format!("taxonomy-{tax}-{slug}.html"));
            if let Some(a) = alias {
                v.push(format!("{a}.html"));
            }
            v.push(format!("taxonomy-{tax}.html"));
            v.push("taxonomy.html".into());
            v.push("archive.html".into());
            v
        }
        TemplateKey::Author { login } => vec![
            format!("author-{login}.html"),
            "author.html".into(),
            "archive.html".into(),
        ],
        TemplateKey::Date => vec!["date.html".into(), "archive.html".into()],
        TemplateKey::Search => vec!["search.html".into()],
        TemplateKey::FrontPage { slug, id } => {
            let mut v = vec!["front-page.html".to_owned()];
            v.extend(page_row(slug, id));
            v
        }
        TemplateKey::FrontPosts => vec!["front-page.html".into(), "home.html".into()],
        TemplateKey::NotFound => vec!["404.html".into()],
    };
    c.push(INDEX.into());
    c
}

/// The Page row of the 06 §2 table, without the final `index.html`.
fn page_row(slug: &str, id: &str) -> Vec<String> {
    vec![
        format!("page-{slug}.html"),
        format!("page-{id}.html"),
        "page.html".into(),
        "single.html".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> String {
        v.to_owned()
    }

    /// Resolves `key` against a theme holding exactly `files`.
    fn pick(key: &TemplateKey, files: &[&str]) -> String {
        resolve_template(key, |n| files.contains(&n))
    }

    /// For each row: with all candidates present the first wins; removing the winner
    /// each time walks the chain in order, ending at `index.html`.
    fn assert_chain(key: TemplateKey, chain: &[&str]) {
        let mut files: Vec<&str> = chain.to_vec();
        for expected in chain {
            assert_eq!(pick(&key, &files), *expected, "{key:?} with {files:?}");
            files.retain(|f| f != expected);
        }
        assert_eq!(pick(&key, &[]), "index.html", "{key:?} with no files");
    }

    #[test]
    fn single() {
        assert_chain(
            TemplateKey::Single {
                kind: s("post"),
                slug: s("hello"),
            },
            &[
                "single-post-hello.html",
                "single-post.html",
                "single.html",
                "index.html",
            ],
        );
    }

    #[test]
    fn page() {
        assert_chain(
            TemplateKey::Page {
                slug: s("about"),
                id: s("0190"),
            },
            &[
                "page-about.html",
                "page-0190.html",
                "page.html",
                "single.html",
                "index.html",
            ],
        );
    }

    #[test]
    fn archive() {
        assert_chain(
            TemplateKey::Archive { kind: s("event") },
            &["archive-event.html", "archive.html", "index.html"],
        );
    }

    #[test]
    fn term_generic_taxonomy() {
        assert_chain(
            TemplateKey::Term {
                tax: s("genre"),
                slug: s("jazz"),
            },
            &[
                "taxonomy-genre-jazz.html",
                "taxonomy-genre.html",
                "taxonomy.html",
                "archive.html",
                "index.html",
            ],
        );
    }

    #[test]
    fn term_category_aliases() {
        assert_chain(
            TemplateKey::Term {
                tax: s("category"),
                slug: s("news"),
            },
            &[
                "category-news.html",
                "taxonomy-category-news.html",
                "category.html",
                "taxonomy-category.html",
                "taxonomy.html",
                "archive.html",
                "index.html",
            ],
        );
    }

    #[test]
    fn term_tag_aliases() {
        for tax in ["tag", "post_tag"] {
            assert_chain(
                TemplateKey::Term {
                    tax: s(tax),
                    slug: s("rust"),
                },
                &[
                    "tag-rust.html",
                    &format!("taxonomy-{tax}-rust.html"),
                    "tag.html",
                    &format!("taxonomy-{tax}.html"),
                    "taxonomy.html",
                    "archive.html",
                    "index.html",
                ],
            );
        }
    }

    #[test]
    fn author() {
        assert_chain(
            TemplateKey::Author { login: s("ana") },
            &[
                "author-ana.html",
                "author.html",
                "archive.html",
                "index.html",
            ],
        );
    }

    #[test]
    fn date() {
        assert_chain(
            TemplateKey::Date,
            &["date.html", "archive.html", "index.html"],
        );
    }

    #[test]
    fn search() {
        assert_chain(TemplateKey::Search, &["search.html", "index.html"]);
    }

    #[test]
    fn front_posts() {
        assert_chain(
            TemplateKey::FrontPosts,
            &["front-page.html", "home.html", "index.html"],
        );
        // Page templates are not for the posts listing.
        assert_eq!(
            pick(&TemplateKey::FrontPosts, &["page.html", "single.html"]),
            "index.html"
        );
    }

    #[test]
    fn front_static_page() {
        let key = TemplateKey::FrontPage {
            slug: s("welcome"),
            id: s("0190"),
        };
        assert_chain(
            key.clone(),
            &[
                "front-page.html",
                "page-welcome.html",
                "page-0190.html",
                "page.html",
                "single.html",
                "index.html",
            ],
        );
        assert_eq!(pick(&key, &["home.html"]), "index.html");
    }

    #[test]
    fn not_found() {
        assert_chain(TemplateKey::NotFound, &["404.html", "index.html"]);
    }

    #[test]
    fn error_has_no_index_fallback() {
        assert_eq!(error_template(|n| n == "500.html"), Some("500.html"));
        assert_eq!(error_template(|n| n == "index.html"), None);
    }

    #[test]
    fn other_types_do_not_match_unrelated_templates() {
        let key = TemplateKey::Single {
            kind: s("event"),
            slug: s("x"),
        };
        assert_eq!(pick(&key, &["single-post.html", "page.html"]), "index.html");
    }
}
