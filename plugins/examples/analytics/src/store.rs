//! What is stored: one JSON blob per UTC day, and the rules that fill it.
//!
//! ```text
//! {"v":1,"views":n,"bots":n,"other":n,
//!  "status":{"2xx":n,…},"paths":{path:n},"referrers":{host:n}}
//! ```

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use serde::{Deserialize, Serialize};

/// The blob format. A blob with a higher version is left alone.
pub const VERSION: u64 = 1;
/// Most distinct paths in a day's `paths` (plus [`OTHER`]).
pub const MAX_PATHS: usize = 500;
/// Most distinct hosts in a day's `referrers` (plus [`OTHER`]).
pub const MAX_REFERRERS: usize = 200;
/// Where counts for keys past the cap go.
pub const OTHER: &str = "(other)";

/// Counts by key. A hash map, not a `BTreeMap`: the tree compares whole strings on the way
/// down (paths share long prefixes) and the fuel is counted in instructions, which made a
/// stats page of 90 blobs cost many times the default limit. The hash is not
/// collision-resistant, which is fine here: a day's map holds at most 500 keys, and the
/// worst a crafted set of paths can do is slow one call down to its fuel limit.
pub type Counts = HashMap<String, u64, BuildHasherDefault<Fx>>;

/// A small multiplicative hasher over 8-byte words.
#[derive(Default)]
pub struct Fx(u64);

impl Fx {
    fn add(&mut self, word: u64) {
        self.0 = (self.0.rotate_left(5) ^ word).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}

impl Hasher for Fx {
    fn write(&mut self, bytes: &[u8]) {
        let (words, rest) = bytes.as_chunks::<8>();
        for word in words {
            self.add(u64::from_le_bytes(*word));
        }
        if !rest.is_empty() {
            let mut w = [0; 8];
            w[..rest.len()].copy_from_slice(rest);
            self.add(u64::from_le_bytes(w));
        }
    }

    fn write_u8(&mut self, byte: u8) {
        self.add(u64::from(byte));
    }

    fn finish(&self) -> u64 {
        // The last step is a multiplication, which leaves the low bits (the bucket index)
        // depending only on the low bits of its input: fold the high half down.
        self.0 ^ (self.0 >> 32)
    }
}

/// One `request.served` event. Only what the plugin uses; `cache` is ignored.
#[derive(Debug, Deserialize)]
pub struct Event {
    pub path: String,
    pub status: u16,
    /// RFC 3339 UTC.
    pub ts: String,
    #[serde(default)]
    pub referrer_host: Option<String>,
    /// `browser`, `bot` or `other`.
    pub ua_class: String,
}

/// A day's counts.
#[derive(Debug, Serialize, Deserialize)]
pub struct Totals {
    pub v: u64,
    /// Browser requests answered 200 or 304.
    #[serde(default)]
    pub views: u64,
    /// Requests whose user agent core classed as a bot, whatever the status.
    #[serde(default)]
    pub bots: u64,
    /// Requests of any other kind of client (no user agent, `curl`, …).
    #[serde(default)]
    pub other: u64,
    /// Every counted response by class: `2xx`, `3xx`, `4xx`, `5xx`.
    #[serde(default)]
    pub status: Counts,
    /// Views by path.
    #[serde(default)]
    pub paths: Counts,
    /// Views by the host of their referrer.
    #[serde(default)]
    pub referrers: Counts,
}

impl Default for Totals {
    fn default() -> Self {
        Self {
            v: VERSION,
            views: 0,
            bots: 0,
            other: 0,
            status: Counts::default(),
            paths: Counts::default(),
            referrers: Counts::default(),
        }
    }
}

/// A stored blob, read back.
#[derive(Debug)]
pub enum Stored {
    Totals(Totals),
    /// Written by a newer version of this plugin: not ours to overwrite.
    Newer,
    /// Not a blob of ours.
    Corrupt,
}

#[derive(Deserialize)]
struct Version {
    v: Option<u64>,
}

/// A day's counts without its maps: the chart and the totals need no more, and skipping
/// the maps costs a fraction of building them.
#[derive(Deserialize)]
struct Brief {
    v: u64,
    #[serde(default)]
    views: u64,
    #[serde(default)]
    bots: u64,
    #[serde(default)]
    other: u64,
    #[serde(default)]
    status: Counts,
    // `paths` and `referrers` are not listed: serde skips unknown fields without building
    // them.
}

impl Stored {
    /// Read a stored blob in full.
    pub fn decode(bytes: &[u8]) -> Self {
        match serde_json::from_slice::<Totals>(bytes) {
            Ok(t) => Self::of_version(t.v, t),
            // A newer version may have changed a field's type: its `v` still tells.
            Err(_) => Self::unreadable(bytes),
        }
    }

    /// Read a stored blob without its `paths` and `referrers`, which come back empty.
    ///
    /// The plugin writes the counts first and `paths` right after them, so the counts are
    /// the text before the first `,"paths":` and the maps are never parsed (not even
    /// skipped: scanning a tens-of-kilobytes blob costs about as much as a tenth of
    /// decoding it, times up to 90 days). A blob in any other layout is read whole.
    pub fn decode_brief(bytes: &[u8]) -> Self {
        const MAPS: &[u8] = b",\"paths\":";
        let head = bytes
            .windows(MAPS.len())
            .position(|w| w == MAPS)
            .map(|at| [&bytes[..at], b"}"].concat());
        // `v` is the first field, so a head that lacks it is another layout: read it whole.
        let brief = head
            .and_then(|head| serde_json::from_slice::<Brief>(&head).ok())
            .map(Ok)
            .unwrap_or_else(|| serde_json::from_slice::<Brief>(bytes));
        match brief {
            Ok(b) => Self::of_version(
                b.v,
                Totals {
                    v: b.v,
                    views: b.views,
                    bots: b.bots,
                    other: b.other,
                    status: b.status,
                    ..Totals::default()
                },
            ),
            Err(_) => Self::unreadable(bytes),
        }
    }

    fn of_version(v: u64, totals: Totals) -> Self {
        match v {
            VERSION => Self::Totals(totals),
            v if v > VERSION => Self::Newer,
            _ => Self::Corrupt,
        }
    }

    fn unreadable(bytes: &[u8]) -> Self {
        match serde_json::from_slice::<Version>(bytes) {
            Ok(Version { v: Some(v) }) if v > VERSION => Self::Newer,
            _ => Self::Corrupt,
        }
    }
}

impl Totals {
    /// Count one event. `self_host` is the site's own host: a visit that came from it is
    /// not a referral.
    pub fn record(&mut self, ev: &Event, self_host: Option<&str>) {
        if let Some(class) = status_class(ev.status) {
            *self.status.entry(class.to_owned()).or_default() += 1;
        }
        match ev.ua_class.as_str() {
            "bot" => self.bots += 1,
            "browser" => {
                if matches!(ev.status, 200 | 304) {
                    self.views += 1;
                    bump(&mut self.paths, &ev.path, MAX_PATHS);
                    if let Some(host) = ev.referrer_host.as_deref()
                        && !host.is_empty()
                        && !self_host.is_some_and(|own| own.eq_ignore_ascii_case(host))
                    {
                        bump(&mut self.referrers, host, MAX_REFERRERS);
                    }
                }
            }
            _ => self.other += 1,
        }
    }
}

fn status_class(status: u16) -> Option<&'static str> {
    match status {
        200..=299 => Some("2xx"),
        300..=399 => Some("3xx"),
        400..=499 => Some("4xx"),
        500..=599 => Some("5xx"),
        _ => None,
    }
}

/// Add one to `key`. A key not yet in the map counts under [`OTHER`] once the map holds
/// `cap` keys besides it.
fn bump(map: &mut Counts, key: &str, cap: usize) {
    if let Some(n) = map.get_mut(key) {
        *n += 1;
        return;
    }
    let real = map.len() - usize::from(map.contains_key(OTHER));
    let key = if real < cap { key } else { OTHER };
    *map.entry(key.to_owned()).or_default() += 1;
}

/// The lowercased host of an `http(s)` URL: no scheme, user info, port or path. An IPv6
/// host keeps its brackets, as the event's `referrer_host` does.
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let host = if authority.starts_with('[') {
        &authority[..=authority.find(']')?]
    } else {
        authority.split(':').next()?
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}
