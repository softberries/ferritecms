//! What is stored: one JSON blob per UTC day, and the rules that fill it.
//!
//! ```text
//! {"v":1,"views":n,"bots":n,"other":n,
//!  "status":{"2xx":n,…},"paths":{path:n},"referrers":{host:n}}
//! ```

use std::collections::HashMap;

use serde::de::{Deserializer, MapAccess, Visitor};
use serde::{Deserialize, Serialize};

/// The blob format. A blob with a higher version is left alone.
pub const VERSION: u64 = 1;
/// Most distinct paths in a day's `paths` (plus [`OTHER`]).
pub const MAX_PATHS: usize = 500;
/// Most distinct hosts in a day's `referrers` (plus [`OTHER`]).
pub const MAX_REFERRERS: usize = 200;
/// Where counts for keys past the cap go.
pub const OTHER: &str = "(other)";
/// Longest path stored, in bytes (cut at a character boundary). Core cuts events at 512;
/// the page shows 200 characters anyway, and the cost of a day is the length of its keys.
pub const MAX_PATH_BYTES: usize = 200;
/// Longest referrer host stored, in bytes. Core allows 253; a real host is shorter, and
/// a referrer is the visitor's to forge.
pub const MAX_HOST_BYTES: usize = 100;

/// Counts by key. A std `HashMap` with its randomly seeded hasher (the seed comes from
/// `wasi:random`, which the host links): a plain deterministic hash would let a visitor who
/// forges `Referer` hosts pick colliding keys. A `BTreeMap` is not an option, because the
/// fuel is counted in instructions and the tree compares whole strings on the way down
/// (paths share long prefixes): many times slower.
pub type Counts = HashMap<String, u64>;

/// One `request.served` event. Only what the plugin uses; `cache` is ignored. The path and
/// the host are cut to [`MAX_PATH_BYTES`] and [`MAX_HOST_BYTES`] as they are read, so a
/// 508-byte path costs a scan and no allocation of its tail: a batch of 1000 of them is
/// the most expensive thing the plugin is asked to do.
#[derive(Debug, Deserialize)]
pub struct Event {
    #[serde(deserialize_with = "clipped_path")]
    pub path: String,
    pub status: u16,
    /// RFC 3339 UTC.
    pub ts: String,
    #[serde(default)]
    pub referrer_host: Option<Host>,
    /// `browser`, `bot` or `other`.
    pub ua_class: String,
}

/// A referrer host, read cut to [`MAX_HOST_BYTES`].
#[derive(Debug)]
pub struct Host(pub String);

impl<'de> Deserialize<'de> for Host {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        clipped(d, MAX_HOST_BYTES).map(Self)
    }
}

fn clipped_path<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    clipped(d, MAX_PATH_BYTES)
}

/// A string cut to `max` bytes on a character boundary.
fn clipped<'de, D: Deserializer<'de>>(d: D, max: usize) -> Result<String, D::Error> {
    struct Cut(usize);
    impl Visitor<'_> for Cut {
        type Value = String;

        fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            f.write_str("a string")
        }

        fn visit_str<E>(self, v: &str) -> Result<String, E> {
            Ok(clip(v, self.0).to_owned())
        }
    }
    d.deserialize_str(Cut(max))
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
    #[serde(default, deserialize_with = "paths_map")]
    pub paths: Counts,
    /// Views by the host of their referrer.
    #[serde(default, deserialize_with = "referrers_map")]
    pub referrers: Counts,
}

fn paths_map<'de, D: Deserializer<'de>>(d: D) -> Result<Counts, D::Error> {
    counts_with_room(d, MAX_PATHS + 1)
}

fn referrers_map<'de, D: Deserializer<'de>>(d: D) -> Result<Counts, D::Error> {
    counts_with_room(d, MAX_REFERRERS + 1)
}

/// A map read with room for `room` keys at the outset. JSON gives no length ahead, so the
/// default would grow the table in steps and hash every key again at each step, and hashing
/// 200-byte keys is most of what reading a busy day costs.
fn counts_with_room<'de, D: Deserializer<'de>>(d: D, room: usize) -> Result<Counts, D::Error> {
    struct Room(usize);
    impl<'de> Visitor<'de> for Room {
        type Value = Counts;

        fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            f.write_str("a map of counts")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Counts, A::Error> {
            let mut out = Counts::with_capacity(self.0);
            while let Some((key, n)) = map.next_entry::<String, u64>()? {
                out.insert(key, n);
            }
            Ok(out)
        }
    }
    d.deserialize_map(Room(room))
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
                    if let Some(Host(host)) = &ev.referrer_host
                        && !host.is_empty()
                        // The host is already cut: compare it with the site's, cut alike.
                        && !self_host.is_some_and(|own| {
                            clip(own, MAX_HOST_BYTES).eq_ignore_ascii_case(host)
                        })
                    {
                        bump(&mut self.referrers, host, MAX_REFERRERS);
                    }
                }
            }
            _ => self.other += 1,
        }
    }
}

/// `s` cut to at most `max` bytes, on a character boundary.
fn clip(s: &str, max: usize) -> &str {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
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
