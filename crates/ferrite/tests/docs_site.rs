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

fn serve(data: &Path, port: u16) -> Serve {
    let mut cmd = ferrite();
    cmd.args(["serve", "--data"])
        .arg(data)
        .env("FERRITE_SERVER__LISTEN", format!("127.0.0.1:{port}"))
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
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
    rx.recv_timeout(Duration::from_secs(60))
        .expect("serve did not report its address");
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

/// Every `name="…"` attribute value in `html`.
fn attr_values(html: &str, name: &str) -> Vec<String> {
    let needle = format!(" {name}=\"");
    html.split(needle.as_str())
        .skip(1)
        .filter_map(|rest| rest.split_once('"').map(|(v, _)| unescape(v)))
        .collect()
}

/// The path of a same-site URL (`/x/` or `http://127.0.0.1:PORT/x/`), without its fragment;
/// `None` for another site, a fragment-only link or another scheme.
fn same_site_path(href: &str, origin: &str) -> Option<String> {
    let rest = if let Some(rest) = href.strip_prefix(origin) {
        if rest.is_empty() { "/" } else { rest }
    } else if href.starts_with('/') && !href.starts_with("//") {
        href
    } else {
        return None;
    };
    let path = rest.split('#').next().unwrap_or("");
    (!path.is_empty()).then(|| path.to_string())
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
            let to_path = same_site_path(&to, origin)
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

/// The ids in the "On this page" nav (`<nav class="docs-toc" …>`) that have no matching
/// `id="…"` on the page.
fn toc_problems(html: &str) -> Vec<String> {
    let Some(start) = html.find("<nav class=\"docs-toc\"") else {
        return Vec::new();
    };
    let nav = &html[start..];
    let nav = &nav[..nav.find("</nav>").expect("toc nav is closed")];
    let ids: BTreeSet<String> = attr_values(html, "id").into_iter().collect();
    attr_values(nav, "href")
        .into_iter()
        .filter_map(|h| h.strip_prefix('#').map(str::to_string))
        .filter(|id| !ids.contains(id))
        .collect()
}

fn h1_count(html: &str) -> usize {
    html.matches("<h1>").count() + html.matches("<h1 ").count()
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

    let _serve = serve(&data, port);

    let index = get(port, "/sitemap.xml");
    assert_eq!(index.status, 200, "/sitemap.xml");
    let mut urls = Vec::new();
    for file in locs(&index.body) {
        let path = same_site_path(&file, &origin).unwrap_or_else(|| panic!("sitemap {file}"));
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
    for url in &urls {
        let path = same_site_path(url, &origin).unwrap_or_else(|| panic!("sitemap URL {url}"));
        let page = get(port, &path);
        if page.status != 200 {
            problems.push(format!("{path}: answers {}", page.status));
            continue;
        }
        let h1 = h1_count(&page.body);
        if h1 != 1 {
            problems.push(format!("{path}: {h1} <h1> elements"));
        }
        for id in toc_problems(&page.body) {
            problems.push(format!("{path}: TOC link #{id} has no matching id"));
        }
        for href in attr_values(&page.body, "href") {
            let Some(target) = same_site_path(&href, &origin) else {
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
    assert!(
        problems.is_empty(),
        "{} problems over {} pages:\n{}",
        problems.len(),
        urls.len(),
        problems.join("\n")
    );
}
