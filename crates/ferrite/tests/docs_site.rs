#![allow(clippy::unwrap_used, clippy::expect_used)]
//! The documentation site (M4-3b P63): `site/content` imported with `ferrite import
//! markdown --front-page` into a fresh SQLite site with the `docs` theme active, then every
//! URL in the sitemap fetched. Each page answers 200 and has one `<h1>`; every same-site
//! link answers 200 (or a 301/308 to a 200); every link in the "On this page" list points
//! to an id on the same page. SQLite only: the test is about the content, not the storage
//! (ruling R-b2).

use std::collections::{BTreeSet, HashMap};
use std::io::{BufRead, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Paths the link check leaves out: theme assets, media files and the admin SPA.
const SKIPPED_PREFIXES: [&str; 3] = ["/_theme/", "/_media/", "/admin/"];

fn ferrite() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ferrite"));
    cmd.env_clear();
    cmd
}

fn content_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../site/content")
        .canonicalize()
        .expect("site/content")
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

/// Runs `cmd`, asserting exit 0, and returns its stdout.
fn ok(mut cmd: Command, what: &str) -> String {
    let out = cmd.output().expect(what);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "{what} exited {:?}\nstdout:\n{stdout}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
    stdout
}

/// `ferrite serve`, killed on drop.
struct Serve(Child);

impl Drop for Serve {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Starts `serve` and waits for its "Listening on" line. Its stderr goes to `log`, which
/// the panic shows when it does not start.
fn serve(data: &Path, port: u16, log: &Path) -> Serve {
    let mut cmd = ferrite();
    cmd.args(["serve", "--data"])
        .arg(data)
        .env("FERRITE_SERVER__LISTEN", format!("127.0.0.1:{port}"))
        .stdout(Stdio::piped())
        .stderr(std::fs::File::create(log).expect("serve log"));
    let mut child = cmd.spawn().expect("spawn serve");
    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            let Ok(line) = line else { return };
            if line.starts_with("Listening on http://") {
                let _ = tx.send(());
            }
        }
    });
    let serve = Serve(child);
    if let Err(e) = rx.recv_timeout(Duration::from_secs(60)) {
        let stderr = std::fs::read_to_string(log).unwrap_or_default();
        panic!("serve did not report its address ({e}); its stderr:\n{stderr}");
    }
    serve
}

/// One HTTP response: the status, the `Location` header and the body.
struct Response {
    status: u16,
    location: Option<String>,
    body: String,
}

/// A plain HTTP/1.0 GET (no keep-alive, no chunking, no compression asked for).
fn get(port: u16, path: &str) -> Response {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\nAccept: text/html,*/*\r\n\r\n"
    )
    .unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).expect("read response");
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or_else(|| panic!("{path}: no header end"));
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let body = String::from_utf8_lossy(&raw[split + 4..]).into_owned();
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap_or_else(|| panic!("{path}: bad status line in {head}"));
    let location = lines.find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case("location")
            .then(|| v.trim().to_string())
    });
    Response {
        status,
        location,
        body,
    }
}

/// The text of every `<loc>` element.
fn locs(xml: &str) -> Vec<String> {
    xml.split("<loc>")
        .skip(1)
        .filter_map(|rest| rest.split_once("</loc>").map(|(v, _)| unescape(v.trim())))
        .collect()
}

/// Decodes the character references templates write into attributes: the five named ones
/// and numeric ones (MiniJinja's auto-escape writes `/` as `&#x2f;`).
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let decoded = rest.find(';').and_then(|end| {
            let name = &rest[1..end];
            let c = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => {
                    let num = name.strip_prefix('#')?;
                    let code = match num.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                        None => num.parse().ok()?,
                    };
                    char::from_u32(code)
                }
            }?;
            Some((c, end + 1))
        });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[test]
fn unescape_decodes_named_and_numeric_references() {
    assert_eq!(unescape("&#x2f;guides&#x2F;a&#47;"), "/guides/a/");
    assert_eq!(
        unescape("a&amp;b &lt;c&gt; &quot;&apos;&#39;"),
        "a&b <c> \"''"
    );
    assert_eq!(unescape("&nope; & &#xZZ;"), "&nope; & &#xZZ;");
}

/// One start tag (`name` lowercase, attributes decoded) or end tag (`name` is `/nav`).
#[derive(Debug, PartialEq)]
struct Tag {
    name: String,
    attrs: Vec<(String, String)>,
}

impl Tag {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// The tags of `html` in document order. Attributes may be double-quoted, single-quoted
/// or unquoted and separated by any whitespace, newlines included. Only real markup is
/// read: comments, `<!doctype>` and the text inside `<script>` and `<style>` are skipped,
/// and escaped text (`&lt;a href=…` in a code block) is not a tag.
fn tags(html: &str) -> Vec<Tag> {
    let b = html.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(off) = html[i..].find('<') {
        i += off + 1;
        if html[i..].starts_with("!--") {
            i = html[i..].find("-->").map_or(b.len(), |e| i + e + 3);
            continue;
        }
        let end_tag = b.get(i) == Some(&b'/');
        let start = if end_tag { i + 1 } else { i };
        let mut j = start;
        while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'-') {
            j += 1;
        }
        if j == start || !b[start].is_ascii_alphabetic() {
            continue;
        }
        let name = html[start..j].to_ascii_lowercase();
        let mut attrs = Vec::new();
        loop {
            while j < b.len() && (b[j].is_ascii_whitespace() || b[j] == b'/') {
                j += 1;
            }
            if j >= b.len() || b[j] == b'>' {
                j += 1;
                break;
            }
            let n0 = j;
            while j < b.len() && !b[j].is_ascii_whitespace() && !b"=>/".contains(&b[j]) {
                j += 1;
            }
            let attr_name = html[n0..j].to_ascii_lowercase();
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            let mut value = String::new();
            if b.get(j) == Some(&b'=') {
                j += 1;
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                match b.get(j) {
                    Some(&q) if q == b'"' || q == b'\'' => {
                        let v0 = j + 1;
                        let v1 = html[v0..].find(q as char).map_or(b.len(), |e| v0 + e);
                        value = unescape(&html[v0..v1]);
                        j = (v1 + 1).min(b.len());
                    }
                    _ => {
                        let v0 = j;
                        while j < b.len() && !b[j].is_ascii_whitespace() && b[j] != b'>' {
                            j += 1;
                        }
                        value = unescape(&html[v0..j]);
                    }
                }
            }
            if !attr_name.is_empty() {
                attrs.push((attr_name, value));
            }
        }
        i = j.min(b.len());
        if !end_tag && (name == "script" || name == "style") {
            // Byte offsets survive ASCII lowercasing, so `</SCRIPT>` ends the text too.
            let close = format!("</{name}");
            i = html[i..]
                .to_ascii_lowercase()
                .find(&close)
                .map_or(b.len(), |e| i + e);
        }
        out.push(Tag {
            name: if end_tag { format!("/{name}") } else { name },
            attrs,
        });
    }
    out
}

#[test]
fn tags_reads_every_attribute_form_and_only_markup() {
    let html = "<!doctype html><!-- <a href=\"/c\"> --><a\nhref='/s' id=x><A HREF=/u>\
                <link rel=\"x\"\thref=\"&#x2f;e&#x2f;\"></a><script>if (a<b) {}</SCRIPT>\
                <pre>&lt;a href=\"/no\"&gt;</pre><a href=\"/after\">";
    let hrefs: Vec<String> = tags(html)
        .iter()
        .filter_map(|t| t.attr("href").map(str::to_string))
        .collect();
    assert_eq!(hrefs, ["/s", "/u", "/e/", "/after"]);
    assert!(tags(html).iter().any(|t| t.name == "/a"));
    assert_eq!(tags(html)[0].attr("id"), Some("x"));
}

/// Where `href` on the page at `page_path` points on this site: a path with its query,
/// without its fragment. Root-relative (`/x/`), relative (`x/`, `../x/`, `?q`), the site's
/// origin (`http://127.0.0.1:PORT/x/`) and protocol-relative to it (`//127.0.0.1:PORT/x/`)
/// are this site; `None` for a fragment-only link, another site or another scheme.
fn resolve(href: &str, page_path: &str, origin: &str) -> Option<String> {
    let href = href.split('#').next().unwrap_or("");
    if href.is_empty() {
        return None;
    }
    let host = origin.split_once("://").map_or(origin, |(_, h)| h);
    let path = if let Some(rest) = href.strip_prefix(origin) {
        match rest {
            "" => "/".to_string(),
            r if r.starts_with(['/', '?']) => r.to_string(),
            _ => return None,
        }
    } else if let Some(rest) = href.strip_prefix("//") {
        match rest.strip_prefix(host)? {
            "" => "/".to_string(),
            r if r.starts_with(['/', '?']) => r.to_string(),
            _ => return None,
        }
    } else if href.starts_with('/') {
        href.to_string()
    } else if href
        .split(['/', '?'])
        .next()
        .is_some_and(|first| first.contains(':'))
    {
        // `mailto:`, `https://other.example/`, …
        return None;
    } else if let Some(query) = href.strip_prefix('?') {
        let base = page_path.split('?').next().unwrap_or("/");
        return Some(format!("{base}?{query}"));
    } else {
        let base = page_path.split('?').next().unwrap_or("/");
        let dir = &base[..base.rfind('/').map_or(0, |i| i + 1)];
        format!("{dir}{href}")
    };
    let (p, query) = match path.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (path, None),
    };
    let mut segs: Vec<&str> = Vec::new();
    for s in p.split('/').skip(1) {
        match s {
            "." => {}
            ".." => {
                segs.pop();
            }
            s => segs.push(s),
        }
    }
    let mut out = format!("/{}", segs.join("/"));
    if (p.ends_with("/.") || p.ends_with("/..")) && !out.ends_with('/') {
        out.push('/');
    }
    if let Some(q) = query {
        out.push('?');
        out.push_str(&q);
    }
    Some(out)
}

#[test]
fn resolve_handles_every_link_form() {
    let o = "http://127.0.0.1:9";
    let page = "/guides/themes/";
    assert_eq!(resolve("/a/", page, o).as_deref(), Some("/a/"));
    assert_eq!(resolve("x/", page, o).as_deref(), Some("/guides/themes/x/"));
    assert_eq!(resolve("../x/", page, o).as_deref(), Some("/guides/x/"));
    assert_eq!(resolve("../../", page, o).as_deref(), Some("/"));
    assert_eq!(
        resolve("?s=a", page, o).as_deref(),
        Some("/guides/themes/?s=a")
    );
    assert_eq!(resolve("http://127.0.0.1:9", page, o).as_deref(), Some("/"));
    assert_eq!(resolve("../..", page, o).as_deref(), Some("/"));
    assert_eq!(resolve("x/..", page, o).as_deref(), Some("/guides/themes/"));
    assert_eq!(
        resolve("http://127.0.0.1:9/a/#b", page, o).as_deref(),
        Some("/a/")
    );
    assert_eq!(resolve("//127.0.0.1:9/a/", page, o).as_deref(), Some("/a/"));
    assert_eq!(resolve("//127.0.0.1:90/a/", page, o), None);
    assert_eq!(resolve("http://127.0.0.1:90/a/", page, o), None);
    assert_eq!(resolve("https://example.com/", page, o), None);
    assert_eq!(resolve("mailto:a@example.com", page, o), None);
    assert_eq!(resolve("#top", page, o), None);
}

/// The status a link ends at: its own, or that of the target of one 301/308.
fn final_status(port: u16, origin: &str, path: &str) -> Result<(), String> {
    let r = get(port, path);
    match r.status {
        200 => Ok(()),
        301 | 308 => {
            let to = r
                .location
                .ok_or_else(|| format!("{} without Location", r.status))?;
            let to_path = resolve(&to, path, origin)
                .ok_or_else(|| format!("{} to another site: {to}", r.status))?;
            let t = get(port, &to_path);
            if t.status == 200 {
                Ok(())
            } else {
                Err(format!("{} to {to}, which answers {}", r.status, t.status))
            }
        }
        other => Err(format!("answers {other}")),
    }
}

/// What one page holds for the checks.
struct PageScan {
    h1: usize,
    /// `h2`/`h3` elements with an `id` (the body's headings; the docs theme's own
    /// headings have none).
    headings: usize,
    /// The `href`s inside `<nav class="docs-toc" …>`, `None` when the page has no such nav.
    toc: Option<Vec<String>>,
    ids: BTreeSet<String>,
    hrefs: Vec<String>,
}

fn scan(html: &str) -> PageScan {
    let mut p = PageScan {
        h1: 0,
        headings: 0,
        toc: None,
        ids: BTreeSet::new(),
        hrefs: Vec::new(),
    };
    let mut in_toc = false;
    for t in tags(html) {
        if let Some(id) = t.attr("id") {
            p.ids.insert(id.to_string());
        }
        match t.name.as_str() {
            "h1" => p.h1 += 1,
            "h2" | "h3" if t.attr("id").is_some() => p.headings += 1,
            "nav"
                if t.attr("class")
                    .is_some_and(|c| c.split_whitespace().any(|c| c == "docs-toc")) =>
            {
                in_toc = true;
                p.toc.get_or_insert_with(Vec::new);
            }
            "/nav" => in_toc = false,
            _ => {}
        }
        if let Some(href) = t.attr("href") {
            p.hrefs.push(href.to_string());
            if in_toc && let Some(toc) = p.toc.as_mut() {
                toc.push(href.to_string());
            }
        }
    }
    p
}

#[test]
fn every_docs_page_answers_with_one_h1_and_working_links() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("site");
    let port = free_port();
    let origin = format!("http://127.0.0.1:{port}");

    let mut init = ferrite();
    init.args(["init", "--title", "FerriteCMS", "--url", &origin, "--data"])
        .arg(&data);
    ok(init, "init");
    let mut user = ferrite();
    user.args([
        "user",
        "create",
        "--login",
        "docs",
        "--email",
        "docs@example.com",
        "--data",
    ])
    .arg(&data);
    ok(user, "user create");
    let mut import = ferrite();
    import
        .args(["import", "markdown"])
        .arg(content_dir())
        .args(["--front-page", "--source", "ferrite-docs", "--data"])
        .arg(&data);
    // Exit 0 only: a skipped file, a missing image or a broken link (exit 2) fails.
    let report = ok(import, "import markdown");
    let mut activate = ferrite();
    activate
        .args(["theme", "activate", "--data"])
        .arg(&data)
        .arg("docs");
    ok(activate, "theme activate docs");

    let _serve = serve(&data, port, &tmp.path().join("serve.log"));

    let index = get(port, "/sitemap.xml");
    assert_eq!(index.status, 200, "/sitemap.xml");
    let mut urls = Vec::new();
    for file in locs(&index.body) {
        let path = resolve(&file, "/", &origin).unwrap_or_else(|| panic!("sitemap {file}"));
        let part = get(port, &path);
        assert_eq!(part.status, 200, "{file}");
        urls.extend(locs(&part.body));
    }
    assert!(
        urls.len() >= 20,
        "the sitemap lists {} URLs, expected at least 20: {urls:#?}\nimport:\n{report}",
        urls.len()
    );

    let mut checked: HashMap<String, Result<(), String>> = HashMap::new();
    let mut problems = Vec::new();
    let mut with_toc = 0;
    let root_listed = urls
        .iter()
        .any(|u| resolve(u, "/", &origin).as_deref() == Some("/"));
    let mut front_permalink = None;
    for url in &urls {
        let mut path = resolve(url, "/", &origin).unwrap_or_else(|| panic!("sitemap URL {url}"));
        let mut page = get(port, &path);
        // M4-4 P67: the front page's own permalink answers 301 to `/`. Until the sitemap
        // names the front page `/` itself (M4-4 Task 3), it lists that permalink: then,
        // and only for a redirect to `/`, `/` is checked in its place.
        if page.status == 301
            && page.location.as_deref() == Some("/")
            && !root_listed
            && front_permalink.is_none()
        {
            front_permalink = Some(path);
            path = "/".to_owned();
            page = get(port, &path);
        }
        if page.status != 200 {
            problems.push(format!("{path}: answers {}", page.status));
            continue;
        }
        // The sidebar (the page tree) links the front page as `/`, never its permalink.
        let tree_links: Vec<String> = tags(&page.body)
            .iter()
            .filter(|t| t.name == "a" && t.attr("class") == Some("tree__link"))
            .filter_map(|t| t.attr("href"))
            .filter_map(|href| resolve(href, &path, &origin))
            .collect();
        if !tree_links.iter().any(|l| l == "/") {
            problems.push(format!("{path}: the sidebar does not link `/`"));
        }
        if let Some(p) = &front_permalink
            && tree_links.contains(p)
        {
            problems.push(format!("{path}: the sidebar links the front page as {p}"));
        }
        let scan = scan(&page.body);
        if scan.h1 != 1 {
            problems.push(format!("{path}: {} <h1> elements", scan.h1));
        }
        match &scan.toc {
            Some(toc) => {
                with_toc += 1;
                for href in toc {
                    match href.strip_prefix('#') {
                        Some(id) if scan.ids.contains(id) => {}
                        _ => problems
                            .push(format!("{path}: TOC link {href} is not an id on the page")),
                    }
                }
            }
            // The docs theme shows "On this page" for two or more level 2 and 3
            // headings (`show_toc` is on by default), so a missing nav is a regression.
            None if scan.headings >= 2 => problems.push(format!(
                "{path}: {} h2/h3 headings but no <nav class=\"docs-toc\">",
                scan.headings
            )),
            None => {}
        }
        for href in &scan.hrefs {
            let Some(target) = resolve(href, &path, &origin) else {
                continue;
            };
            if SKIPPED_PREFIXES.iter().any(|p| target.starts_with(p)) {
                continue;
            }
            let result = checked
                .entry(target.clone())
                .or_insert_with(|| final_status(port, &origin, &target));
            if let Err(e) = result {
                problems.push(format!("{path}: link {href}: {e}"));
            }
        }
    }
    // The sidebar links every page, so a checker that misread the hrefs (an escaped `/`)
    // would see far fewer targets than pages.
    assert!(
        checked.len() >= urls.len(),
        "only {} distinct same-site links were checked over {} pages: {:?}",
        checked.len(),
        urls.len(),
        checked.keys().collect::<BTreeSet<_>>()
    );
    // The TOC check above must not pass because no page had a TOC.
    assert!(with_toc >= 1, "no page has a <nav class=\"docs-toc\">");
    assert!(
        problems.is_empty(),
        "{} problems over {} pages:\n{}",
        problems.len(),
        urls.len(),
        problems.join("\n")
    );
}
