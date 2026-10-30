//! What is stored: one JSON blob per UTC day, and the rules that fill it.
//!
//! A day within the detail window keeps its maps, under `day:YYYY-MM-DD`:
//!
//! ```text
//! {"v":1,"views":n,"bots":n,"other":n,
//!  "status":{"2xx":n,…},"paths":{path:n},"referrers":{host:n}}
//! ```
//!
//! An older day is compacted to its counts alone ([`Summary`]), under `totals:YYYY-MM-DD`:
//!
//! ```text
//! {"v":1,"views":n,"bots":n,"other":n,"status":{"2xx":n,…}}
//! ```

use std::borrow::Cow;
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

/// One `request.served` event. Only what the plugin uses; `cache` is ignored. The strings
/// borrow from the payload (core's have no JSON escape, so nothing is copied until a key is
/// new to a map), and the path and the host are cut to [`MAX_PATH_BYTES`] and
/// [`MAX_HOST_BYTES`] as they are read, so a 508-byte path costs a scan: a batch of 1000 of
/// them is the most expensive thing the plugin is asked to do.
#[derive(Debug, Deserialize)]
pub struct Event<'a> {
    #[serde(borrow, deserialize_with = "clipped_path")]
    pub path: Cow<'a, str>,
    pub status: u16,
    /// RFC 3339 UTC.
    #[serde(borrow)]
    pub ts: Cow<'a, str>,
    #[serde(default, borrow)]
    pub referrer_host: Option<Host<'a>>,
    /// `browser`, `bot` or `other`.
    #[serde(borrow)]
    pub ua_class: Cow<'a, str>,
}

/// A referrer host, read cut to [`MAX_HOST_BYTES`].
#[derive(Debug)]
pub struct Host<'a>(pub Cow<'a, str>);

impl<'de: 'a, 'a> Deserialize<'de> for Host<'a> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        clipped(d, MAX_HOST_BYTES).map(Self)
    }
}

/// A path cut to [`MAX_PATH_BYTES`], and then before a `%` or `%X` the cut left at its
/// end: core sends paths percent-encoded, and a key must not end in half an escape.
fn clipped_path<'de: 'a, 'a, D: Deserializer<'de>>(d: D) -> Result<Cow<'a, str>, D::Error> {
    let path = clipped(d, MAX_PATH_BYTES)?;
    let b = path.as_bytes();
    let partial = match b.len() {
        n if n >= 1 && b[n - 1] == b'%' => 1,
        n if n >= 2 && b[n - 2] == b'%' => 2,
        _ => 0,
    };
    let end = path.len() - partial;
    Ok(match path {
        Cow::Borrowed(p) => Cow::Borrowed(&p[..end]),
        Cow::Owned(mut p) => {
            p.truncate(end);
            Cow::Owned(p)
        }
    })
}

/// A string cut to `max` bytes on a character boundary: borrowed from the input when it
/// holds no escape, else decoded and copied.
fn clipped<'de: 'a, 'a, D: Deserializer<'de>>(d: D, max: usize) -> Result<Cow<'a, str>, D::Error> {
    struct Cut(usize);
    impl<'de> Visitor<'de> for Cut {
        type Value = Cow<'de, str>;

        fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            f.write_str("a string")
        }

        fn visit_borrowed_str<E>(self, v: &'de str) -> Result<Self::Value, E> {
            Ok(Cow::Borrowed(clip(v, self.0)))
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> {
            Ok(Cow::Owned(clip(v, self.0).to_owned()))
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

/// A day's counts without its maps: what a compacted day keeps, and what the page reads of
/// a day whose lists it does not show (the chart and the totals need no more, and skipping
/// the maps costs a fraction of building them).
#[derive(Debug, Serialize, Deserialize)]
pub struct Summary {
    pub v: u64,
    #[serde(default)]
    pub views: u64,
    #[serde(default)]
    pub bots: u64,
    #[serde(default)]
    pub other: u64,
    #[serde(default)]
    pub status: Counts,
    // `paths` and `referrers` are not listed: serde skips unknown fields without building
    // them.
}

impl From<Summary> for Totals {
    fn from(s: Summary) -> Self {
        Totals {
            v: s.v,
            views: s.views,
            bots: s.bots,
            other: s.other,
            status: s.status,
            ..Totals::default()
        }
    }
}

/// The bytes a compacted blob may hold; one this plugin writes is at most 212. A bigger one
/// was not written by it, and is not decoded.
pub const MAX_SUMMARY_BYTES: usize = 1024;
/// How far into a day blob [`Stored::decode_head`] looks for the end of the counts.
const HEAD_SCAN_BYTES: usize = 1024;
/// A day blob this small is read whole by [`Stored::decode_head`] when its counts do not
/// come first.
const SMALL_BLOB_BYTES: usize = 4096;
/// Where the counts of a day blob end: the plugin writes them first and `paths` next.
const MAPS: &[u8] = b",\"paths\":";

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
        let brief = Self::counts_before(bytes, bytes.len())
            .map(Ok)
            .unwrap_or_else(|| serde_json::from_slice::<Summary>(bytes));
        match brief {
            Ok(s) => Self::of_version(s.v, s.into()),
            Err(_) => Self::unreadable(bytes),
        }
    }

    /// Read the counts of a day blob for compaction, at a bounded cost whatever is stored:
    /// the counts before a `,"paths":` within the first [`HEAD_SCAN_BYTES`], else a blob of
    /// at most [`SMALL_BLOB_BYTES`] read whole. Anything else is not in the layout this
    /// plugin writes: [`Stored::Newer`] when it starts with a higher `{"v":`, else
    /// [`Stored::Corrupt`].
    pub fn decode_head(bytes: &[u8]) -> Self {
        let summary = Self::counts_before(bytes, HEAD_SCAN_BYTES).or_else(|| {
            (bytes.len() <= SMALL_BLOB_BYTES)
                .then(|| serde_json::from_slice::<Summary>(bytes).ok())
                .flatten()
        });
        match summary {
            Some(s) => Self::of_version(s.v, s.into()),
            None if bytes.len() <= SMALL_BLOB_BYTES => Self::unreadable(bytes),
            None => match leading_version(bytes) {
                Some(v) if v > VERSION => Self::Newer,
                _ => Self::Corrupt,
            },
        }
    }

    /// Read a compacted blob (a [`Summary`]). One over [`MAX_SUMMARY_BYTES`] is not this
    /// plugin's and is not parsed.
    pub fn decode_summary(bytes: &[u8]) -> Self {
        if bytes.len() > MAX_SUMMARY_BYTES {
            return match leading_version(bytes) {
                Some(v) if v > VERSION => Self::Newer,
                _ => Self::Corrupt,
            };
        }
        match serde_json::from_slice::<Summary>(bytes) {
            Ok(s) => Self::of_version(s.v, s.into()),
            Err(_) => Self::unreadable(bytes),
        }
    }

    /// The counts before the first `,"paths":` in the first `scan` bytes, parsed. `v` is
    /// the first field, so a head that lacks it is another layout: `None`.
    fn counts_before(bytes: &[u8], scan: usize) -> Option<Summary> {
        let at = bytes[..bytes.len().min(scan)]
            .windows(MAPS.len())
            .position(|w| w == MAPS)?;
        serde_json::from_slice::<Summary>(&[&bytes[..at], b"}"].concat()).ok()
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
    /// The counts alone, encoded: the blob of a compacted day.
    pub fn summary_blob(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec(&Summary {
            v: VERSION,
            views: self.views,
            bots: self.bots,
            other: self.other,
            status: self.status.clone(),
        })
        .map_err(|e| e.to_string())
    }

    /// Add the counts of `other` (not its maps).
    pub fn add_counts(&mut self, other: &Totals) {
        self.views = self.views.saturating_add(other.views);
        self.bots = self.bots.saturating_add(other.bots);
        self.other = self.other.saturating_add(other.other);
        for (class, n) in &other.status {
            let sum = self.status.entry(class.clone()).or_default();
            *sum = sum.saturating_add(*n);
        }
    }

    /// Count one event. `self_host` is the site's own host: a visit that came from it is
    /// not a referral.
    pub fn record(&mut self, ev: &Event<'_>, self_host: Option<&str>) {
        if let Some(class) = status_class(ev.status) {
            // `get_mut` first: the class is nearly always there, and `entry` takes an owned
            // key, an allocation for every event.
            match self.status.get_mut(class) {
                Some(n) => *n += 1,
                None => {
                    self.status.insert(class.to_owned(), 1);
                }
            }
        }
        match ev.ua_class.as_ref() {
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

/// The version a blob states first (`{"v":N`), read without parsing the rest.
fn leading_version(bytes: &[u8]) -> Option<u64> {
    let rest = bytes.strip_prefix(b"{\"v\":")?;
    let digits = rest.iter().take_while(|c| c.is_ascii_digit()).count();
    std::str::from_utf8(&rest[..digits]).ok()?.parse().ok()
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
    if real < cap {
        map.insert(key.to_owned(), 1);
    } else if let Some(n) = map.get_mut(OTHER) {
        // Full: into the overflow bucket, without an owned key.
        *n += 1;
    } else {
        map.insert(OTHER.to_owned(), 1);
    }
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
