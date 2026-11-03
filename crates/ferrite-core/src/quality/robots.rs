//! A small robots.txt parser (RFC 9309) for the external link checks (design 16 §6.3):
//! the groups of `ferrite-linkcheck` (else `*`), `Allow` and `Disallow` with `*` and `$`,
//! the longest match wins and `Allow` wins a tie (§2.2.2). Input is untrusted: it is cut
//! at [`MAX_ROBOTS`] bytes, read as lossy UTF-8, and at most [`MAX_RULES`] rules are
//! kept.

/// The most of a robots.txt that is read.
pub const MAX_ROBOTS: usize = 64 * 1024;
/// The most rules kept for the matching group.
pub const MAX_RULES: usize = 1_000;
/// Our product token.
pub const AGENT: &str = "ferrite-linkcheck";

#[derive(Debug, Clone, PartialEq, Eq)]
struct Rule {
    allow: bool,
    pattern: String,
}

/// The rules that apply to us.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rules {
    rules: Vec<Rule>,
}

/// Parses `bytes` (cut at [`MAX_ROBOTS`]).
pub fn parse(bytes: &[u8]) -> Rules {
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_ROBOTS)]);
    let mut ours: Vec<Rule> = Vec::new();
    let mut star: Vec<Rule> = Vec::new();
    let mut have_ours = false;
    // The agents of the current group, and whether its rules started.
    let mut agents: Vec<String> = Vec::new();
    let mut in_rules = false;
    for raw in text.split(['\n', '\r']) {
        let line = raw.split('#').next().unwrap_or("").trim();
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        match key.as_str() {
            "user-agent" => {
                if in_rules {
                    agents.clear();
                    in_rules = false;
                }
                agents.push(value.to_ascii_lowercase());
            }
            "allow" | "disallow" => {
                in_rules = true;
                let rule = Rule {
                    allow: key == "allow",
                    pattern: value.to_owned(),
                };
                if agents.iter().any(|a| a == AGENT) {
                    have_ours = true;
                    if ours.len() < MAX_RULES {
                        ours.push(rule.clone());
                    }
                }
                if agents.iter().any(|a| a == "*") && star.len() < MAX_RULES {
                    star.push(rule);
                }
            }
            _ => {}
        }
    }
    Rules {
        rules: if have_ours { ours } else { star },
    }
}

impl Rules {
    /// Whether `path` (with its query) may be fetched. An empty `Disallow` allows all.
    pub fn allows(&self, path: &str) -> bool {
        let mut best: Option<(usize, bool)> = None;
        for r in &self.rules {
            if r.pattern.is_empty() {
                continue;
            }
            if matches(&r.pattern, path) {
                let len = r.pattern.len();
                best = match best {
                    Some((l, a)) if l > len || (l == len && a) => Some((l, a)),
                    _ => Some((len, r.allow)),
                };
            }
        }
        best.is_none_or(|(_, allow)| allow)
    }
}

/// Whether `pattern` (`*` any run, `$` the end when last) matches a prefix of `path`.
/// Iterative, linear in `path` per `*`.
fn matches(pattern: &str, path: &str) -> bool {
    let (pattern, anchored) = match pattern.strip_suffix('$') {
        Some(p) => (p, true),
        None => (pattern, false),
    };
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut pos = 0usize;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            if !path.starts_with(part) {
                return false;
            }
            pos = part.len();
            continue;
        }
        if i == parts.len() - 1 && anchored {
            return path.len() >= pos + part.len() && path.ends_with(part);
        }
        match path[pos..].find(part) {
            Some(at) => pos += at + part.len(),
            None => return false,
        }
    }
    !anchored || pos == path.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_and_precedence() {
        let r = parse(b"User-agent: *\nDisallow: /private/\nAllow: /private/ok\n");
        assert!(r.allows("/"));
        assert!(!r.allows("/private/x"));
        assert!(r.allows("/private/ok/x"));
        // Our own group wins over `*`.
        let r =
            parse(b"User-agent: *\nDisallow: /\n\nUser-agent: ferrite-linkcheck\nDisallow: /x\n");
        assert!(r.allows("/a"));
        assert!(!r.allows("/x/y"));
        // Several agents share a group.
        let r = parse(b"User-agent: a\nUser-agent: *\nDisallow: /b\n");
        assert!(!r.allows("/b"));
        // An empty Disallow allows everything; a tie goes to Allow.
        assert!(parse(b"User-agent: *\nDisallow:\n").allows("/x"));
        assert!(parse(b"User-agent: *\nDisallow: /a\nAllow: /a\n").allows("/a"));
        assert!(parse(b"").allows("/anything"));
    }

    #[test]
    fn wildcards() {
        let r = parse(b"user-agent: *\ndisallow: /*.pdf$\ndisallow: /tmp*/x # comment\n");
        assert!(!r.allows("/docs/a.pdf"));
        assert!(r.allows("/docs/a.pdf?x=1"));
        assert!(!r.allows("/tmp123/x/y"));
        assert!(r.allows("/tmp123/y"));
        assert!(matches("/a$", "/a"));
        assert!(!matches("/a$", "/ab"));
        assert!(matches("*", "/anything"));
    }

    #[test]
    fn robots_parser_hostile() {
        // 1 MiB without a newline, invalid UTF-8, 10 000 groups.
        let long = vec![b'a'; 1024 * 1024];
        assert!(parse(&long).allows("/"));
        assert!(parse(&[0xff, 0xfe, b'\n', 0x80]).allows("/"));
        let mut many = String::new();
        for i in 0..10_000 {
            many.push_str(&format!("User-agent: *\nDisallow: /p{i}\n"));
        }
        let r = parse(many.as_bytes());
        assert!(r.rules.len() <= MAX_RULES);
        // Cut at 64 KiB: a rule past it is not seen.
        let mut big = "User-agent: *\n".to_owned();
        big.push_str(&"#".repeat(MAX_ROBOTS));
        big.push_str("\nDisallow: /\n");
        assert!(parse(big.as_bytes()).allows("/x"));
        let stars = format!("User-agent: *\nDisallow: /{}x\n", "*".repeat(5000));
        let start = std::time::Instant::now();
        assert!(parse(stars.as_bytes()).allows(&"a".repeat(2000)));
        assert!(start.elapsed().as_secs() < 5);
    }
}
