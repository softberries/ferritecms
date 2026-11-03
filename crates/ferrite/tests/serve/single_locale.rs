//! The golden one-locale snapshot (M6-1 Task 1, design 13 §9): the public output of the
//! three starter themes, their feeds, sitemaps and `robots.txt` on a one-locale site,
//! captured before multilingual content existed. Every later change must keep a
//! one-locale site byte-identical, so this test compares each response, after
//! [`normalize`], with the fixture in `tests/fixtures/golden-single-locale/<theme>/`.
//!
//! `FERRITE_UPDATE_GOLDEN=1` (re)writes the fixtures. The values that differ between
//! runs are normalised: UUIDs, long hex runs (media and theme hashes), dates in the
//! formats the themes, feeds and sitemaps print, and `?v=` cache busters. The docs
//! profile imports the small Markdown folder `docs-src/` next to the fixtures (not
//! `site/content`, whose text changes with the documentation; ruling R-1-1).

use std::path::{Path, PathBuf};

use crate::harness::{Profile, Server};

const PASSWORD: &str = "correct horse battery staple";

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const SHORT_MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/golden-single-locale")
}

fn digits(b: &[u8], at: usize, n: usize) -> bool {
    b.len() >= at + n && b[at..at + n].iter().all(u8::is_ascii_digit)
}

/// The length of an ISO 8601 / RFC 3339 date (`2026-11-02`, optionally with a time,
/// fraction and offset) starting at `at`, or 0.
fn iso_len(b: &[u8], at: usize) -> usize {
    if !(digits(b, at, 4)
        && b.get(at + 4) == Some(&b'-')
        && digits(b, at + 5, 2)
        && b.get(at + 7) == Some(&b'-')
        && digits(b, at + 8, 2))
    {
        return 0;
    }
    let mut i = at + 10;
    if b.get(i) == Some(&b'T')
        && digits(b, i + 1, 2)
        && b.get(i + 3) == Some(&b':')
        && digits(b, i + 4, 2)
    {
        i += 6;
        if b.get(i) == Some(&b':') && digits(b, i + 1, 2) {
            i += 3;
            if b.get(i) == Some(&b'.') {
                i += 1;
                while b.get(i).is_some_and(u8::is_ascii_digit) {
                    i += 1;
                }
            }
        }
        if b.get(i) == Some(&b'Z') {
            i += 1;
        } else if matches!(b.get(i), Some(b'+' | b'-'))
            && digits(b, i + 1, 2)
            && b.get(i + 3) == Some(&b':')
            && digits(b, i + 4, 2)
        {
            i += 6;
        }
    }
    i - at
}

/// The length of an RFC 2822 date (`Mon, 02 Nov 2026 10:00:00 +0000`) at `at`, or 0.
fn rfc2822_len(s: &str, at: usize) -> usize {
    let rest = &s[at..];
    let Some(day) = WEEKDAYS.iter().find(|d| rest.starts_with(*d)) else {
        return 0;
    };
    let b = rest.as_bytes();
    let mut i = day.len();
    if !rest[i..].starts_with(", ") {
        return 0;
    }
    i += 2;
    let d = if digits(b, i, 2) {
        2
    } else if digits(b, i, 1) {
        1
    } else {
        return 0;
    };
    i += d + 1;
    let Some(m) = SHORT_MONTHS.iter().find(|m| rest[i..].starts_with(*m)) else {
        return 0;
    };
    i += m.len() + 1;
    if !(digits(b, i, 4)
        && b.get(i + 4) == Some(&b' ')
        && digits(b, i + 5, 2)
        && b.get(i + 7) == Some(&b':')
        && digits(b, i + 8, 2)
        && b.get(i + 10) == Some(&b':')
        && digits(b, i + 11, 2))
    {
        return 0;
    }
    i += 13;
    if rest[i..].starts_with(" GMT") {
        i += 4;
    } else if b.get(i) == Some(&b' ')
        && matches!(b.get(i + 1), Some(b'+' | b'-'))
        && digits(b, i + 2, 4)
    {
        i += 6;
    }
    i
}

/// The length of a theme date (`2 November 2026`, `2 November 2026, 10:00` or
/// `November 2026`) at `at`, or 0.
fn human_len(s: &str, at: usize) -> usize {
    let rest = &s[at..];
    let b = rest.as_bytes();
    let mut i = 0;
    if digits(b, 0, 2) && b.get(2) == Some(&b' ') {
        i = 3;
    } else if digits(b, 0, 1) && b.get(1) == Some(&b' ') {
        i = 2;
    }
    let Some(m) = MONTHS.iter().find(|m| rest[i..].starts_with(*m)) else {
        return 0;
    };
    i += m.len();
    if !(b.get(i) == Some(&b' ') && digits(b, i + 1, 4)) {
        return 0;
    }
    i += 5;
    if rest[i..].starts_with(", ")
        && digits(b, i + 2, 2)
        && b.get(i + 4) == Some(&b':')
        && digits(b, i + 5, 2)
    {
        i += 7;
    }
    i
}

fn is_hex(c: u8) -> bool {
    c.is_ascii_digit() || (b'a'..=b'f').contains(&c)
}

/// The length of a lowercase hyphenated UUID at `at`, or 0.
fn uuid_len(b: &[u8], at: usize) -> usize {
    let groups = [8, 4, 4, 4, 12];
    let mut i = at;
    for (n, len) in groups.iter().enumerate() {
        if b.len() < i + len || !b[i..i + len].iter().all(|c| is_hex(*c)) {
            return 0;
        }
        i += len;
        if n < 4 {
            if b.get(i) != Some(&b'-') {
                return 0;
            }
            i += 1;
        }
    }
    i - at
}

/// Replaces what differs between runs with placeholders: UUIDs → `<uuid>`, runs of 16
/// or more lowercase hex characters (media and theme hashes) → `<hash>`, dates →
/// `<date>`, `?v=…` values → `?v=<v>`.
pub fn normalize(body: &str) -> String {
    let b = body.as_bytes();
    let mut out = String::with_capacity(body.len());
    let mut i = 0;
    while i < b.len() {
        // Only at a token start, so the middle of a longer word is never a match.
        let start = i == 0 || !b[i - 1].is_ascii_alphanumeric();
        if start {
            let n = uuid_len(b, i);
            if n > 0 && !b.get(i + n).is_some_and(u8::is_ascii_alphanumeric) {
                out.push_str("<uuid>");
                i += n;
                continue;
            }
            let n = iso_len(b, i);
            if n > 0 {
                out.push_str("<date>");
                i += n;
                continue;
            }
            let n = rfc2822_len(body, i);
            if n > 0 {
                out.push_str("<date>");
                i += n;
                continue;
            }
            let n = human_len(body, i);
            if n > 0 {
                out.push_str("<date>");
                i += n;
                continue;
            }
            let hex = b[i..].iter().take_while(|c| is_hex(**c)).count();
            if hex >= 16 && !b.get(i + hex).is_some_and(u8::is_ascii_alphanumeric) {
                out.push_str("<hash>");
                i += hex;
                continue;
            }
        }
        if body[i..].starts_with("?v=") && !body[i..].starts_with("?v=<v>") {
            out.push_str("?v=<v>");
            i += 3;
            while b
                .get(i)
                .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_'))
            {
                i += 1;
            }
            continue;
        }
        let ch = body[i..].chars().next().expect("char");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// The fixture file name of route `path`.
fn fixture_name(path: &str) -> String {
    let mut name: String = path
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    while name.contains("__") {
        name = name.replace("__", "_");
    }
    let name = name.trim_matches('_');
    let b = path.as_bytes();
    if name.is_empty() {
        "root.txt".to_owned()
    } else if path.len() == 6 && digits(b, 1, 4) && b[5] == b'/' {
        // The year archive: named without the year, which moves with the seed's dates.
        "year.txt".to_owned()
    } else if path.bytes().any(|c| c.is_ascii_uppercase()) {
        // Distinct from the lowercase route on a case-insensitive file system.
        format!("upper-{}.txt", name.to_ascii_lowercase())
    } else {
        format!("{name}.txt")
    }
}

/// Compares `body` (normalised, with its status line) with the fixture, or writes the
/// fixture under `FERRITE_UPDATE_GOLDEN=1`.
pub fn assert_golden(theme: &str, name: &str, body: &str) {
    let path = fixtures().join(theme).join(name);
    let got = normalize(body);
    if std::env::var("FERRITE_UPDATE_GOLDEN").as_deref() == Ok("1") {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("fixture dir");
        std::fs::write(&path, &got).expect("write fixture");
        return;
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e} (FERRITE_UPDATE_GOLDEN=1 writes it)",
            path.display()
        )
    });
    if got != want {
        let line = got
            .lines()
            .zip(want.lines())
            .position(|(a, b)| a != b)
            .unwrap_or_else(|| got.lines().count().min(want.lines().count()));
        panic!(
            "{theme}/{name} differs from the golden output at line {}:\n got: {:?}\nwant: {:?}",
            line + 1,
            got.lines().nth(line),
            want.lines().nth(line),
        );
    }
}

/// The `/…` paths of the `<loc>` entries in an XML body.
fn locs(xml: &str) -> Vec<String> {
    xml.split("<loc>")
        .skip(1)
        .filter_map(|p| p.split("</loc>").next())
        .map(|u| {
            let rest = u.split_once("://").map_or(u, |(_, r)| r);
            rest.find('/')
                .map_or("/".to_owned(), |i| rest[i..].to_owned())
        })
        .collect()
}

/// The first year in a `datetime="YYYY…"` attribute of `html`.
fn first_year(html: &str) -> String {
    html.split("datetime=\"")
        .nth(1)
        .map(|r| r[..4].to_owned())
        .expect("a datetime attribute")
}

/// Fetches every route, every sitemap file the index names and every page those files
/// list, and compares each (status, content type, `Location` and body) with its fixture.
fn snapshot(server: &Server, theme: &str, routes: &[String]) {
    let mut routes = routes.to_vec();
    routes.push("/sitemap.xml".to_owned());
    let index = server.get("/sitemap.xml");
    assert_eq!(index.status, 200, "{index:?}");
    for file in locs(&index.text()) {
        let listed = server.get(&file);
        assert_eq!(listed.status, 200, "{file}: {listed:?}");
        routes.push(file);
        routes.extend(locs(&listed.text()));
    }
    let mut seen = std::collections::HashSet::new();
    routes.retain(|r| seen.insert(r.clone()));
    for path in &routes {
        let r = server.get(path);
        let mut text = format!("{} {}\n", r.status, r.header("content-type").unwrap_or("-"));
        if let Some(loc) = r.header("location") {
            text.push_str(&format!("location: {loc}\n"));
        }
        text.push('\n');
        text.push_str(&r.text());
        assert_golden(theme, &fixture_name(path), &text);
    }
}

fn seed(p: &Profile, extra: &[&str]) {
    let mut args = vec!["seed", "demo"];
    args.extend_from_slice(extra);
    let out = p.cmd(&args).output().expect("run seed");
    assert!(out.status.success(), "seed: {out:?}");
}

fn activate(p: &Profile, theme: &str) {
    let out = p
        .cmd(&["theme", "activate", theme])
        .output()
        .expect("activate");
    assert!(out.status.success(), "activate: {out:?}");
}

#[test]
fn golden_blog() {
    let p = Profile::solo();
    p.init();
    seed(&p, &[]);
    let server = Server::start(&p);
    let year = first_year(&server.get("/").text());
    let routes: Vec<String> = [
        "/",
        "/welcome-to-ferritecms/",
        "/writing-your-first-plugin/",
        "/about/",
        "/about/team/",
        "/About/Team",
        "/contact/",
        "/page/2/",
        "/category/news/",
        "/category/guides/",
        "/tag/rust/",
        "/author/admin/",
        "/?s=plugin",
        "/no-such-page/",
        "/feed/",
        "/category/news/feed/",
        "/robots.txt",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .chain([format!("/{year}/")])
    .collect();
    snapshot(&server, "blog", &routes);
}

#[test]
fn golden_business() {
    let p = Profile::solo();
    p.init();
    seed(&p, &["--profile", "business"]);
    activate(&p, "business");
    let server = Server::start(&p);
    let news = server.get("/category/news/").text();
    let year = first_year(&news);
    let routes: Vec<String> = [
        "/",
        "/home/",
        "/home/garden-design/",
        "/home/consulting/",
        "/about/",
        "/contact/",
        "/post/",
        "/category/news/",
        "/author/admin/",
        "/?s=garden",
        "/no-such-page/",
        "/feed/",
        "/category/news/feed/",
        "/robots.txt",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .chain([format!("/{year}/")])
    .collect();
    snapshot(&server, "business", &routes);
}

#[test]
fn golden_docs() {
    let p = Profile::solo();
    p.init();
    p.create_admin("docs", PASSWORD);
    let src = fixtures().join("docs-src");
    let out = p
        .cmd(&[
            "import",
            "markdown",
            src.to_str().expect("utf-8"),
            "--front-page",
            "--source",
            "golden-docs",
        ])
        .output()
        .expect("import markdown");
    assert!(out.status.success(), "import: {out:?}");
    activate(&p, "docs");
    let server = Server::start(&p);
    let routes: Vec<String> = [
        "/",
        "/guide/",
        "/?s=install",
        "/no-such-page/",
        "/feed/",
        "/robots.txt",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    snapshot(&server, "docs", &routes);
}

/// `seed demo --profile multilingual` (M6-1 Task 12): the blog in English and German,
/// served with `<html lang>`, the language switcher, `hreflang` links, the translated
/// menu and the theme's German strings.
#[test]
fn multilingual_demo_serves_both_locales() {
    let p = Profile::solo();
    p.init();
    seed(&p, &["--profile", "multilingual"]);
    let server = Server::start(&p);
    let de = server.get("/de/");
    assert_eq!(de.status, 200, "{de:?}");
    let html = de.text();
    assert!(html.contains("<html lang=\"de\">"), "{html}");
    assert!(html.contains("Willkommen bei FerriteCMS"), "{html}");
    assert!(!html.contains(">Welcome to FerriteCMS<"), "{html}");
    assert!(html.contains("class=\"language-switcher\""), "{html}");
    assert!(html.contains("Zum Inhalt springen"), "{html}");
    assert!(
        html.contains("Hauptnavigation") || html.contains(">Kontakt<"),
        "{html}"
    );
    let about = server.get("/de/about/");
    assert_eq!(about.status, 200, "{about:?}");
    let about = about.text();
    assert!(about.contains("Über uns"), "{about}");
    assert!(
        about.contains("hreflang=\"en\"") && about.contains("hreflang=\"x-default\""),
        "{about}"
    );
    let en = server.get("/about/").text();
    assert!(en.contains("<html lang=\"en\">"), "{en}");
    assert!(en.contains("hreflang=\"de\""), "{en}");
    assert!(en.contains("Skip to content"), "{en}");
    let feed = server.get("/de/feed/").text();
    assert!(feed.contains("<language>de</language>"), "{feed}");
    assert_eq!(server.get("/de/robots.txt").status, 404);
}

#[test]
fn normalize_is_stable() {
    let s = "id 0192f0a4-1b2c-7d3e-8f40-123456789abc at 2026-11-02T10:00:00.123Z \
             or Mon, 02 Nov 2026 10:00:00 +0000 shown 2 November 2026, 10:00 in \
             November 2026 /_media/0123456789abcdef0123/x.png?v=0.3.0 deadbeef";
    let n = normalize(s);
    assert_eq!(
        n,
        "id <uuid> at <date> or <date> shown <date> in <date> /_media/<hash>/x.png?v=<v> deadbeef"
    );
    assert_eq!(normalize(&n), n);
}
