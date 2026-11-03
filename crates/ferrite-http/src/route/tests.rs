#![allow(clippy::unwrap_used)]

use super::*;
use ferrite_core::PermalinkRules;

fn types() -> Vec<TypeInfo> {
    let t = |name: &str, hierarchical, has_archive| TypeInfo {
        name: name.into(),
        hierarchical,
        has_archive,
    };
    vec![
        t("post", false, true),
        t("page", true, false),
        t("book", true, true),
        t("event", false, false),
    ]
}

fn table_with(post: Option<&str>) -> RouteTable {
    let rules = PermalinkRules::from_options(post, None).unwrap();
    RouteTable::new(&rules, &types(), &["category".into(), "tag".into()])
}

fn table() -> RouteTable {
    table_with(None)
}

fn single(kind: &str, parts: &[&str]) -> Candidate {
    Candidate::Single {
        kind: kind.into(),
        parts: parts.iter().map(|p| (*p).to_owned()).collect(),
    }
}

/// Page then post: the default structures share `/<slug>/` (ruling R3).
fn root(slug: &str) -> Route {
    Route::Content(vec![single("page", &[slug]), single("post", &[slug])])
}

fn canonical(s: &str) -> Route {
    Route::Canonical(s.into())
}

fn split(raw: &str) -> (&str, Option<&str>) {
    match raw.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (raw, None),
    }
}

fn check(table: &RouteTable, cases: &[(&str, Route)]) {
    for (raw, want) in cases {
        let (path, query) = split(raw);
        assert_eq!(&resolve(path, query, table), want, "{raw:?}");
    }
}

const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[test]
fn front_search_and_pagination() {
    check(
        &table(),
        &[
            ("/", Route::Front { page: 1 }),
            ("/page/2/", Route::Front { page: 2 }),
            ("/page/1000/", Route::Front { page: 1000 }),
            ("/page/1/", canonical("/")),
            ("/page/2", canonical("/page/2/")),
            ("/PAGE/2/", canonical("/page/2/")),
            ("/page/0/", Route::NotFound),
            ("/page/01/", Route::NotFound),
            ("/page/1001/", Route::NotFound),
            ("/page/abc/", Route::NotFound),
            ("/page/-2/", Route::NotFound),
            ("/page/+2/", Route::NotFound),
            ("/page/99999999999999/", Route::NotFound),
            ("/page/1/?s=x", canonical("/?s=x")),
            (
                "/?s=hello",
                Route::Search {
                    q: "hello".into(),
                    page: 1,
                },
            ),
            (
                "/?s=a+b%21&utm=1",
                Route::Search {
                    q: "a b!".into(),
                    page: 1,
                },
            ),
            (
                "/page/3/?s=hi",
                Route::Search {
                    q: "hi".into(),
                    page: 3,
                },
            ),
            ("/?s=", Route::Front { page: 1 }),
            ("/?s=+++", Route::Front { page: 1 }),
            ("/?s=%ff", Route::Front { page: 1 }),
            ("/?utm_source=x", Route::Front { page: 1 }),
            // `s` only means search at the root.
            ("/about/?s=x", root("about")),
            // Single items do not paginate.
            ("/about/page/2/", Route::NotFound),
            ("/feed/page/2/", Route::NotFound),
        ],
    );
}

#[test]
fn singles_and_canonical_form() {
    check(
        &table(),
        &[
            ("/about/", root("about")),
            ("/about", canonical("/about/")),
            ("/About/", canonical("/about/")),
            ("/ABOUT", canonical("/about/")),
            ("/about/?utm=1", root("about")),
            ("/About?x=1&y=2", canonical("/about/?x=1&y=2")),
            ("/%41bout/", canonical("/about/")),
            ("/abo%75t/", canonical("/about/")),
            (
                "/parent/child/",
                Route::Content(vec![single("page", &["parent", "child"])]),
            ),
            ("/a~b/", root("a~b")),
            ("/a%7Eb/", canonical("/a~b/")),
            ("/a&b/", canonical("/a%26b/")),
            ("/a%26b/", root("a&b")),
            ("/caf%C3%A9/", root("café")),
            // Unicode capitals fold like `slug::validate` does.
            ("/caf%C3%89/", canonical("/caf%C3%A9/")),
            ("/ZAŻÓŁĆ/", canonical("/za%C5%BC%C3%B3%C5%82%C4%87/")),
            // No Unicode normalisation: NFD `e` + U+0301 is another path than NFC `é`.
            ("/cafe%CC%81/", root("cafe\u{301}")),
            // Type-prefixed items: hierarchical and not.
            (
                "/book/vol/ch/",
                Route::Content(vec![
                    single("page", &["book", "vol", "ch"]),
                    single("book", &["vol", "ch"]),
                ]),
            ),
            ("/Book/Vol/", canonical("/book/vol/")),
            (
                "/event/x/",
                Route::Content(vec![
                    single("page", &["event", "x"]),
                    single("event", &["x"]),
                ]),
            ),
            (
                "/event/a/b/",
                Route::Content(vec![single("page", &["event", "a", "b"])]),
            ),
            // No archive: `/event/` is only a slug.
            ("/event/", root("event")),
            // `/post/x/` is not a post URL (posts use their own structure).
            (
                "/post/x/",
                Route::Content(vec![single("page", &["post", "x"])]),
            ),
        ],
    );
}

#[test]
fn unicode_slug_has_one_canonical_form() {
    let canon = "/za%C5%BC%C3%B3%C5%82%C4%87/";
    check(
        &table(),
        &[
            ("/zażółć/", canonical(canon)),
            ("/za%c5%bc%c3%b3%c5%82%c4%87/", canonical(canon)),
            ("/za%c5%bc%C3%B3%c5%82%C4%87", canonical(canon)),
            ("/za%C5%BC%C3%B3%C5%82%C4%87/", root("zażółć")),
            ("/zażółć/?a=1", canonical(&format!("{canon}?a=1"))),
        ],
    );
}

#[test]
fn archives_terms_authors_dates_feeds() {
    let term = |taxonomy: &str, slug: &str, page| Route::Term {
        taxonomy: taxonomy.into(),
        slug: slug.into(),
        page,
    };
    let author = |login: &str, page| Route::Author {
        login: login.into(),
        page,
    };
    check(
        &table(),
        &[
            ("/category/news/", term("category", "news", 1)),
            ("/category/news/page/2/", term("category", "news", 2)),
            ("/category/news/page/1/", canonical("/category/news/")),
            ("/category/news", canonical("/category/news/")),
            ("/Category/News/", canonical("/category/news/")),
            ("/tag/rust/", term("tag", "rust", 1)),
            ("/tag/caf%C3%A9/", term("tag", "café", 1)),
            ("/tag/caf%c3%a9/", canonical("/tag/caf%C3%A9/")),
            (
                "/category/news/feed/",
                Route::Feed(FeedSource::Term {
                    taxonomy: "category".into(),
                    slug: "news".into(),
                }),
            ),
            ("/category/News/feed", canonical("/category/news/feed/")),
            ("/feed/", Route::Feed(FeedSource::Kind("post".into()))),
            ("/feed", canonical("/feed/")),
            ("/Feed/", canonical("/feed/")),
            // Logins keep their case; the fixed word does not.
            ("/author/Admin/", author("Admin", 1)),
            ("/author/Admin/page/3/", author("Admin", 3)),
            ("/AUTHOR/Admin/", canonical("/author/Admin/")),
            ("/author/a%20b/", author("a b", 1)),
            (
                "/2024/",
                Route::Date {
                    year: 2024,
                    month: None,
                    page: 1,
                },
            ),
            (
                "/2024/05/",
                Route::Date {
                    year: 2024,
                    month: Some(5),
                    page: 1,
                },
            ),
            (
                "/2024/05/page/2/",
                Route::Date {
                    year: 2024,
                    month: Some(5),
                    page: 2,
                },
            ),
            ("/2024", canonical("/2024/")),
            // Not a month: falls through to the permalink matcher.
            (
                "/2024/13/",
                Route::Content(vec![single("page", &["2024", "13"])]),
            ),
            (
                "/2024/5/",
                Route::Content(vec![single("page", &["2024", "5"])]),
            ),
            (
                "/book/",
                Route::Archive {
                    kind: "book".into(),
                    page: 1,
                },
            ),
            (
                "/book/page/4/",
                Route::Archive {
                    kind: "book".into(),
                    page: 4,
                },
            ),
            ("/BOOK/", canonical("/book/")),
            (
                "/post/",
                Route::Archive {
                    kind: "post".into(),
                    page: 1,
                },
            ),
            // `/author/` and `/category/` alone are plain slugs.
            ("/author/", root("author")),
            ("/category/", root("category")),
        ],
    );
}

#[test]
fn files_and_reserved_routes() {
    let media = |name: &str| Route::Media {
        hash: HASH.into(),
        name: name.into(),
    };
    let upper = HASH.to_ascii_uppercase();
    let overlong_theme = format!("/_theme/abc/{}x.css", "d/".repeat(31));
    check(
        &table(),
        &[
            ("/healthz", Route::Health),
            ("/readyz", Route::Ready),
            ("/healthz?x=1", Route::Health),
            ("/healthz/", Route::NotFound),
            ("/HEALTHZ", Route::NotFound),
            ("/readyz/", Route::NotFound),
            ("/robots.txt", Route::Robots),
            ("/robots.txt/", canonical("/robots.txt")),
            ("/Robots.TXT", canonical("/robots.txt")),
            ("/sitemap.xml", Route::Sitemap(SitemapPart::Index)),
            ("/sitemap.xml/", canonical("/sitemap.xml")),
            ("/SITEMAP.xml?a=1", canonical("/sitemap.xml?a=1")),
            (
                "/sitemap-post-1.xml",
                Route::Sitemap(SitemapPart::Kind {
                    kind: "post".into(),
                    n: 1,
                }),
            ),
            (
                "/sitemap-book-12.xml",
                Route::Sitemap(SitemapPart::Kind {
                    kind: "book".into(),
                    n: 12,
                }),
            ),
            ("/sitemap-post-2.xml/", canonical("/sitemap-post-2.xml")),
            ("/Sitemap-Post-2.xml", canonical("/sitemap-post-2.xml")),
            ("/sitemap-post-0.xml", Route::NotFound),
            ("/sitemap-post-01.xml", Route::NotFound),
            ("/sitemap-post-x.xml", Route::NotFound),
            ("/sitemap-post-1001.xml", Route::NotFound),
            ("/sitemap-nope-1.xml", Route::NotFound),
            ("/sitemap-1.xml", Route::NotFound),
            ("/sitemap.xml/page/2/", Route::NotFound),
            (&format!("/_media/{HASH}/photo.jpg"), media("photo.jpg")),
            (&format!("/_media/{HASH}/caf%C3%A9.jpg"), media("café.jpg")),
            (&format!("/_media/{HASH}/caf%c3%a9.jpg"), media("café.jpg")),
            (&format!("/_media/{HASH}/a%2Cb%201.jpg"), media("a,b 1.jpg")),
            (&format!("/_media/{HASH}/Photo.JPG"), media("Photo.JPG")),
            (&format!("/_media/{HASH}/photo.jpg?x=1"), media("photo.jpg")),
            (&format!("/_media/{HASH}/photo.jpg/"), Route::NotFound),
            (&format!("/_media/{HASH}/"), Route::NotFound),
            (&format!("/_media/{HASH}"), Route::NotFound),
            (&format!("/_media/{HASH}/a/b"), Route::NotFound),
            (&format!("/_media/{HASH}/a%2Fb"), Route::NotFound),
            (&format!("/_media/{HASH}/a%5Cb"), Route::NotFound),
            (&format!("/_media/{HASH}/%2e%2e"), Route::NotFound),
            (&format!("/_media/{HASH}/.."), Route::NotFound),
            (&format!("/_media/{HASH}/%00"), Route::NotFound),
            (&format!("/_media/{HASH}/%ff"), Route::NotFound),
            (&format!("/_media/{upper}/x"), Route::NotFound),
            (&format!("/_media/{}/x", &HASH[1..]), Route::NotFound),
            (&format!("/_MEDIA/{HASH}/x"), Route::NotFound),
            ("/_media", Route::NotFound),
            ("/_media/", Route::NotFound),
            (
                "/_theme/abc123/css/site.css",
                Route::Theme {
                    hash: "abc123".into(),
                    path: "css/site.css".into(),
                },
            ),
            (
                "/_theme/abc/f%C3%B3nt.woff2",
                Route::Theme {
                    hash: "abc".into(),
                    path: "fónt.woff2".into(),
                },
            ),
            ("/_theme/abc/", Route::NotFound),
            ("/_theme/abc", Route::NotFound),
            ("/_theme/abc/../x.css", Route::NotFound),
            ("/_theme/abc/%2e%2e/x.css", Route::NotFound),
            ("/_theme/abc/a//b.css", Route::NotFound),
            ("/_theme/abc/a%2Fb.css", Route::NotFound),
            ("/_theme/ABC/x.css", Route::NotFound),
            ("/_theme/xyz/x.css", Route::NotFound),
            (&overlong_theme, Route::NotFound),
            ("/_Theme/abc/x.css", Route::NotFound),
        ],
    );
}

/// `/_preview/<id>`: exactly a lowercase hyphenated UUID, no trailing slash, never
/// redirected; anything else under `/_preview` is 404 (P20, F27).
#[test]
fn preview_paths() {
    const ID: &str = "0192f0c4-7e3a-7b1c-9d2e-3f4a5b6c7d8e";
    let preview = Route::Preview {
        id: ID.parse().unwrap(),
    };
    let upper = ID.to_ascii_uppercase();
    let simple = ID.replace('-', "");
    for t in [
        table(),
        table_with(Some("/%year%/%month%/%slug%/")),
        table_with(Some("/%category%/%slug%/")),
    ] {
        check(
            &t,
            &[
                (&format!("/_preview/{ID}"), preview.clone()),
                (&format!("/_preview/{ID}?x=1"), preview.clone()),
                (&format!("/_preview/{ID}/"), Route::NotFound),
                (&format!("/_preview/{upper}"), Route::NotFound),
                (&format!("/_preview/{simple}"), Route::NotFound),
                (&format!("/_preview/{{{ID}}}"), Route::NotFound),
                (&format!("/_preview/urn:uuid:{ID}"), Route::NotFound),
                (&format!("/_preview/{}", &ID[1..]), Route::NotFound),
                (&format!("/_preview/{ID}0"), Route::NotFound),
                (&format!("/_preview/{ID}/x"), Route::NotFound),
                (&format!("/_preview/%30{}", &ID[1..]), Route::NotFound),
                (&format!("/_PREVIEW/{ID}"), Route::NotFound),
                (&format!("/_Preview/{ID}/"), Route::NotFound),
                ("/_preview/not-an-id", Route::NotFound),
                ("/_preview/", Route::NotFound),
                ("/_preview", Route::NotFound),
                ("/_preview/page/2/", Route::NotFound),
                ("/_preview/feed/", Route::NotFound),
            ],
        );
    }
}

/// Review Focus 3: path tricks are 404 or a canonical 301, never another item.
#[test]
fn path_tricks() {
    let long_ok = format!("/{}/", "a".repeat(2046));
    assert_eq!(long_ok.len(), 2048);
    let long_bad = format!("/{}/", "a".repeat(2047));
    let deep_ok = format!("{}/", "/a".repeat(32));
    let deep_bad = format!("{}/", "/a".repeat(33));
    let deep_parts = vec!["a"; 32];
    check(
        &table(),
        &[
            ("/%2e%2e/", Route::NotFound),
            ("/%2E%2E/", Route::NotFound),
            ("/%2e./", Route::NotFound),
            ("/%2e/", Route::NotFound),
            ("/../", Route::NotFound),
            ("/./", Route::NotFound),
            ("/..", Route::NotFound),
            ("/a/../b/", Route::NotFound),
            ("/a/./b/", Route::NotFound),
            ("/a/%2e%2e/b/", Route::NotFound),
            // Empty interior segments are rejected, not collapsed (no 301 to `/x/`).
            ("//x", Route::NotFound),
            ("//x/", Route::NotFound),
            ("//", Route::NotFound),
            ("/a//b/", Route::NotFound),
            ("/a///", Route::NotFound),
            ("/a\\b/", Route::NotFound),
            ("/a%5cb/", Route::NotFound),
            ("/a%2fb/", Route::NotFound),
            ("/a%2Fb/", Route::NotFound),
            ("/%ff/", Route::NotFound),
            ("/%c3%28/", Route::NotFound),
            ("/%c3/", Route::NotFound),
            ("/%00/", Route::NotFound),
            ("/a%0a/", Route::NotFound),
            ("/a%7f/", Route::NotFound),
            ("/a%c2%85/", Route::NotFound),
            ("/a\u{7}b/", Route::NotFound),
            ("/%zz/", Route::NotFound),
            ("/%a/", Route::NotFound),
            ("/a%/", Route::NotFound),
            // Decoded once: `%252e` is the literal `%2e`, a slug no item can have.
            ("/%252e%252e/", root("%2e%2e")),
            ("", Route::NotFound),
            ("about", Route::NotFound),
            (&long_ok, root(&"a".repeat(2046))),
            (&long_bad, Route::NotFound),
            (&deep_ok, Route::Content(vec![single("page", &deep_parts)])),
            (&deep_bad, Route::NotFound),
        ],
    );
}

#[test]
fn canonical_redirects_stay_on_site() {
    let t = table();
    for raw in [
        "/About", "/%41", "/a%2F", "/A/B", "/Page/1/", "/page/1", "/feed", "/x%2F%2F",
    ] {
        if let Route::Canonical(to) = resolve(raw, None, &t) {
            assert!(
                to.starts_with('/') && !to.starts_with("//"),
                "{raw} -> {to}"
            );
        }
    }
}

#[test]
fn dated_post_structure() {
    let post = |slug: &str, year, month| Candidate::PostByStructure {
        slug: slug.into(),
        year: Some(year),
        month: Some(month),
        category: None,
    };
    check(
        &table_with(Some("/%year%/%month%/%slug%/")),
        &[
            (
                "/2024/05/hello/",
                Route::Content(vec![
                    single("page", &["2024", "05", "hello"]),
                    post("hello", 2024, 5),
                ]),
            ),
            ("/2024/05/Hello", canonical("/2024/05/hello/")),
            // The slug segment tells a post from the month archive.
            (
                "/2024/05/",
                Route::Date {
                    year: 2024,
                    month: Some(5),
                    page: 1,
                },
            ),
            (
                "/2024/",
                Route::Date {
                    year: 2024,
                    month: None,
                    page: 1,
                },
            ),
            (
                "/2024/5/hello/",
                Route::Content(vec![single("page", &["2024", "5", "hello"])]),
            ),
            (
                "/2024/13/hello/",
                Route::Content(vec![single("page", &["2024", "13", "hello"])]),
            ),
            // Posts are no longer at the root.
            ("/hello/", Route::Content(vec![single("page", &["hello"])])),
            ("/2024/05/hello/page/2/", Route::NotFound),
        ],
    );
}

#[test]
fn category_and_literal_post_structures() {
    let by_cat = |slug: &str, cat: &str| Candidate::PostByStructure {
        slug: slug.into(),
        year: None,
        month: None,
        category: Some(cat.into()),
    };
    check(
        &table_with(Some("/%category%/%slug%/")),
        &[
            (
                "/news/hello/",
                Route::Content(vec![
                    single("page", &["news", "hello"]),
                    by_cat("hello", "news"),
                ]),
            ),
            ("/News/Hello/", canonical("/news/hello/")),
            // Static term routes win over the matcher.
            (
                "/category/news/",
                Route::Term {
                    taxonomy: "category".into(),
                    slug: "news".into(),
                    page: 1,
                },
            ),
            ("/hello/", Route::Content(vec![single("page", &["hello"])])),
        ],
    );
    check(
        &table_with(Some("/blog/%slug%/")),
        &[
            (
                "/blog/x/",
                Route::Content(vec![single("page", &["blog", "x"]), single("post", &["x"])]),
            ),
            ("/BLOG/x", canonical("/blog/x/")),
        ],
    );
    // Capitalised literals are rejected by core (ruling R13), so every candidate of a
    // path shares one canonical spelling.
    assert!(PermalinkRules::from_options(Some("/Blog/%slug%/"), None).is_err());
}

/// Ruling R33: `s` is cut to [`MAX_SEARCH_CHARS`] characters on a char boundary, and
/// the cut form is the canonical one (route and cache key).
#[test]
fn search_query_is_capped() {
    use super::MAX_SEARCH_CHARS;
    let long = "ż".repeat(MAX_SEARCH_CHARS + 50);
    let q = NormalQuery::parse(Some(&format!("s={long}")));
    let s = q.s.clone().unwrap();
    assert_eq!(s.chars().count(), MAX_SEARCH_CHARS);
    assert!(long.starts_with(&s));
    // Any longer query with the same first 200 characters shares the cache key.
    let other = NormalQuery::parse(Some(&format!("s={long}xyz")));
    assert_eq!(other.cache_query(), q.cache_query());
    // A cut that ends in spaces is trimmed again.
    let spaced = format!("{}{}", "a".repeat(MAX_SEARCH_CHARS - 1), "+".repeat(10));
    let q = NormalQuery::parse(Some(&format!("s={spaced}b")));
    assert_eq!(q.s.unwrap(), "a".repeat(MAX_SEARCH_CHARS - 1));
    let exact = "b".repeat(MAX_SEARCH_CHARS);
    assert_eq!(
        NormalQuery::parse(Some(&format!("s={exact}"))).s.unwrap(),
        exact
    );
    match resolve("/", Some(&format!("s={long}")), &table()) {
        Route::Search { q, .. } => assert_eq!(q.chars().count(), MAX_SEARCH_CHARS),
        other => panic!("{other:?}"),
    }
}

#[test]
fn normal_query() {
    let q = NormalQuery::parse(Some("utm=1&s=a+b%20c&page=2&s=ignored&cpage=3&cpage=4"));
    assert_eq!(q.s.as_deref(), Some("a b c"));
    assert_eq!(q.page, Some(2));
    assert_eq!(q.cpage, Some(3));
    assert_eq!(q.cache_query(), "cpage=3&page=2&s=a%20b%20c");
    assert_eq!(
        NormalQuery::parse(Some("cpage=4294967295")).cpage,
        Some(u32::MAX)
    );
    assert_eq!(NormalQuery::parse(None), NormalQuery::default());
    assert_eq!(NormalQuery::parse(Some("")).cache_query(), "");
    for raw in [
        "page=01",
        "page=0",
        "page=1",
        "page=x",
        "page=1001",
        "s=",
        "s=%ff",
        "s=%00",
        "cpage=1",
        "cpage=0",
        "cpage=01",
        "cpage=abc",
        "cpage=-2",
        "cpage=4294967296",
    ] {
        assert_eq!(
            NormalQuery::parse(Some(raw)),
            NormalQuery::default(),
            "{raw}"
        );
    }
    assert_eq!(NormalQuery::parse(Some("s=+x+")).s.as_deref(), Some("x"));
    assert_eq!(
        NormalQuery::parse(Some("s=%26%3D")).cache_query(),
        "s=%26%3D"
    );
}

#[test]
fn redirect_query_is_visible_ascii() {
    let t = table();
    check(
        &t,
        &[
            ("/About?q=ż", canonical("/about/?q=%C5%BC")),
            ("/About?a=%41&b=c#d", canonical("/about/?a=%41&b=c%23d")),
            ("/About?a b", canonical("/about/?a%20b")),
        ],
    );
    // Not reachable through hyper, but `resolve` is a pure function: CR/LF must never
    // reach a `Location` header.
    assert_eq!(
        resolve("/About", Some("x=\r\nSet-Cookie: a=b"), &t),
        canonical("/about/?x=%0D%0ASet-Cookie:%20a=b")
    );
    assert_eq!(
        resolve("/About", Some("\u{7f}\u{0}"), &t),
        canonical("/about/?%7F%00")
    );
}

#[test]
fn overlong_canonical_is_not_found() {
    // 1000 raw `ż` are 2002 bytes; encoded they are 6001.
    let raw = format!("/{}", "ż".repeat(1000));
    assert!(raw.len() <= MAX_PATH_BYTES);
    assert_eq!(resolve(&raw, None, &table()), Route::NotFound);
}

/// Seeded fuzzing: every redirect target is a safe, visible-ASCII, local path that
/// resolves without a further redirect or a 404.
#[test]
fn canonical_targets_are_safe_and_stable() {
    const TOKENS: &[&str] = &[
        "%",
        "a",
        "f",
        "0",
        "9",
        "e",
        "%2e",
        "%2E",
        "%2F",
        "%2f",
        ".",
        "..",
        ";",
        "+",
        "ż",
        "%C5%BC",
        "%c5%bc",
        "%c0%ae",
        "page",
        "PAGE",
        "2",
        "01",
        "1",
        "2024",
        "05",
        "13",
        "Blog",
        "blog",
        "category",
        "Tag",
        "feed",
        "author",
        "book",
        "event",
        "sitemap.xml",
        "sitemap-post-1.xml",
        "robots.TXT",
        "_media",
        "healthz",
        "\\",
        "~",
        "%7e",
        "-",
        "\u{212A}",
        "%E2%84%AA",
        "\u{130}",
        "É",
        "%41",
        "%00",
        " ",
        "%20",
        "?",
        "#",
    ];
    const QUERIES: &[&str] = &["", "s=x", "s=ż", "a=\r\n", "b=#c", "page=2&s=+", "%zz"];
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = |n: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % n as u64) as usize
    };
    let tables = [
        table(),
        table_with(Some("/%year%/%month%/%slug%/")),
        table_with(Some("/%category%/%slug%/")),
        table_with(Some("/blog/%slug%/")),
    ];
    let mut redirects = 0;
    for i in 0..20_000 {
        let t = &tables[i % tables.len()];
        let mut path = String::new();
        for _ in 0..1 + next(6) {
            path.push('/');
            if next(20) == 0 {
                continue; // an empty segment now and then
            }
            for _ in 0..1 + next(3) {
                path.push_str(TOKENS[next(TOKENS.len())]);
            }
        }
        if next(2) == 0 {
            path.push('/');
        }
        let query = match QUERIES[next(QUERIES.len())] {
            "" => None,
            q => Some(q),
        };
        let Route::Canonical(target) = resolve(&path, query, t) else {
            continue;
        };
        redirects += 1;
        let ctx = format!("{path:?} {query:?} -> {target:?}");
        assert!(target.starts_with('/'), "{ctx}");
        assert!(
            !target.starts_with("//") && !target.starts_with("/\\"),
            "{ctx}"
        );
        assert!(target.bytes().all(|b| (0x21..=0x7E).contains(&b)), "{ctx}");
        assert!(target.len() <= MAX_PATH_BYTES + 64, "{ctx}");
        let (p, q) = split(&target);
        assert!(p.len() <= MAX_PATH_BYTES, "{ctx}");
        let again = resolve(p, q, t);
        assert!(
            !matches!(again, Route::Canonical(_) | Route::NotFound),
            "{ctx} -> {again:?}"
        );
    }
    assert!(redirects > 1000, "only {redirects} redirects generated");
}

/// A table for a site in `en` (default), `de` and `pt-BR`.
fn table_localized() -> RouteTable {
    let locales = ferrite_domain::locale::SiteLocales::from_option(&serde_json::json!([
        {"tag": "en"}, {"tag": "de"}, {"tag": "pt-BR"}
    ]))
    .unwrap();
    let rules = PermalinkRules::default().with_locales(locales);
    RouteTable::new(&rules, &types(), &["category".into(), "tag".into()])
}

fn check_localized(table: &RouteTable, cases: &[(&str, Option<&str>, Route)]) {
    for (raw, tag, want) in cases {
        let (path, query) = split(raw);
        let (locale, route) = resolve_localized(path, query, table);
        assert_eq!(&route, want, "{raw:?}");
        if !matches!(route, Route::NotFound) {
            assert_eq!(locale.tag.as_deref(), *tag, "{raw:?}");
        }
    }
}

#[test]
fn prefix_routes_content_listing_feed_search() {
    let t = table_localized();
    check_localized(
        &t,
        &[
            ("/de/", Some("de"), Route::Front { page: 1 }),
            ("/de/page/2/", Some("de"), Route::Front { page: 2 }),
            ("/de/about/", Some("de"), root("about")),
            (
                "/de/about/team/",
                Some("de"),
                Route::Content(vec![single("page", &["about", "team"])]),
            ),
            (
                "/de/feed/",
                Some("de"),
                Route::Feed(FeedSource::Kind("post".into())),
            ),
            (
                "/de/category/nachrichten/",
                Some("de"),
                Route::Term {
                    taxonomy: "category".into(),
                    slug: "nachrichten".into(),
                    page: 1,
                },
            ),
            (
                "/de/?s=garten",
                Some("de"),
                Route::Search {
                    q: "garten".into(),
                    page: 1,
                },
            ),
            ("/pt-br/sobre/", Some("pt-BR"), root("sobre")),
            ("/about/", None, root("about")),
            ("/", None, Route::Front { page: 1 }),
            // The default locale's own tag is no prefix.
            ("/en/", None, root("en")),
        ],
    );
}

#[test]
fn prefix_canonicalises_case_and_slash() {
    let t = table_localized();
    check_localized(
        &t,
        &[
            ("/de", Some("de"), canonical("/de/")),
            ("/de?s=x", Some("de"), canonical("/de/?s=x")),
            ("/DE/about/", Some("de"), canonical("/de/about/")),
            ("/DE/about", Some("de"), canonical("/de/about/")),
            ("/de/about", Some("de"), canonical("/de/about/")),
            ("/de/page/1/", Some("de"), canonical("/de/")),
            ("/PT-BR/", Some("pt-BR"), canonical("/pt-br/")),
        ],
    );
}

#[test]
fn prefix_with_files_or_internal_routes_is_404() {
    let t = table_localized();
    for raw in [
        "/de/robots.txt",
        "/de/sitemap.xml",
        "/de/sitemap-post-1.xml",
        &format!("/de/_media/{HASH}/a.png"),
        &format!("/de/_theme/{HASH}/style.css"),
        "/de/_preview/0192f0a4-1b2c-7d3e-8f40-123456789abc",
        "/de/healthz",
        "/de/readyz",
        "/de/_comment",
    ] {
        let (path, query) = split(raw);
        assert_eq!(
            resolve_localized(path, query, &t).1,
            Route::NotFound,
            "{raw}"
        );
    }
    // Without a prefix they are what they always were.
    assert_eq!(resolve_localized("/robots.txt", None, &t).1, Route::Robots);
}

#[test]
fn unknown_two_letter_segment_is_content() {
    let t = table_localized();
    check_localized(
        &t,
        &[(
            "/fr/about/",
            None,
            Route::Content(vec![single("page", &["fr", "about"])]),
        )],
    );
}

/// With no locale prefixes `resolve_localized` is `resolve`, for many generated paths.
#[test]
fn no_locales_means_resolve_unchanged() {
    let t = table();
    let mut seed = 7u64;
    let mut next = |n: usize| {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        ((seed >> 33) as usize) % n
    };
    const TOKENS: [&str; 12] = [
        "de", "DE", "about", "page", "2", "feed", "category", "news", "2024", "%41", "x", "en",
    ];
    for _ in 0..200 {
        let mut path = String::new();
        for _ in 0..1 + next(4) {
            path.push('/');
            path.push_str(TOKENS[next(TOKENS.len())]);
        }
        if next(2) == 0 {
            path.push('/');
        }
        let (locale, route) = resolve_localized(&path, None, &t);
        assert_eq!(locale, RouteLocale::default(), "{path}");
        assert_eq!(route, resolve(&path, None, &t), "{path}");
    }
}
