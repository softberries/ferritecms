//! Problem collection and typed accessors over a `toml::Table`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ferrite_domain::Secret;
use toml::{Table, Value};

use crate::error::ConfigProblem;

pub(crate) struct Cx {
    pub problems: Vec<ConfigProblem>,
    /// Settings that are valid but have no effect (`Config::warnings`).
    pub warnings: Vec<ConfigProblem>,
    pub data_dir: PathBuf,
    /// Dotted config path -> env var name, for every key (and section prefix) set by env.
    pub env_paths: HashMap<String, String>,
    /// Dotted config path -> raw env string, so a string key set to `2024` or `true` still works.
    pub env_raw: HashMap<String, String>,
}

pub(crate) fn join(base: &str, key: &str) -> String {
    if base.is_empty() {
        key.to_owned()
    } else {
        format!("{base}.{key}")
    }
}

impl Cx {
    pub fn push(&mut self, path: &str, message: impl Into<String>) {
        let path = match self.env_paths.get(path) {
            Some(var) => format!("env:{var}"),
            None => path.to_owned(),
        };
        self.problems.push(ConfigProblem {
            path,
            message: message.into(),
        });
    }

    pub fn unknown(&mut self, table: Table, base: &str) {
        for key in table.keys() {
            self.push(&join(base, key), "unknown key");
        }
    }

    /// Remove a sub-table; a non-table value is reported and treated as absent.
    pub fn section(&mut self, t: &mut Table, base: &str, key: &str) -> Option<Table> {
        match t.remove(key)? {
            Value::Table(sub) => Some(sub),
            _ => {
                self.push(&join(base, key), "expected a table");
                None
            }
        }
    }

    pub fn string(&mut self, t: &mut Table, base: &str, key: &str) -> Option<String> {
        let path = join(base, key);
        let value = t.remove(key)?;
        // String keys set from the environment take the raw text, unparsed.
        if let Some(raw) = self.env_raw.get(&path) {
            return Some(raw.clone());
        }
        match value {
            Value::String(s) => Some(s),
            _ => {
                self.push(&path, "expected a string");
                None
            }
        }
    }

    pub fn boolean(&mut self, t: &mut Table, base: &str, key: &str) -> Option<bool> {
        let path = join(base, key);
        match t.remove(key)? {
            Value::Boolean(b) => Some(b),
            _ => {
                self.push(&path, "expected true or false");
                None
            }
        }
    }

    /// Integer >= 0.
    pub fn uint(&mut self, t: &mut Table, base: &str, key: &str) -> Option<u64> {
        let path = join(base, key);
        match t.remove(key)? {
            Value::Integer(i) if i >= 0 => Some(i as u64),
            _ => {
                self.push(&path, "expected a non-negative integer");
                None
            }
        }
    }

    /// Integer >= 1.
    pub fn positive(&mut self, t: &mut Table, base: &str, key: &str) -> Option<u64> {
        let v = self.uint(t, base, key)?;
        if v == 0 {
            self.push(&join(base, key), "must be greater than zero");
            return None;
        }
        Some(v)
    }

    /// Integer >= 1 that fits in `u32`.
    pub fn positive_u32(&mut self, t: &mut Table, base: &str, key: &str) -> Option<u32> {
        let v = self.positive(t, base, key)?;
        match u32::try_from(v) {
            Ok(v) => Some(v),
            Err(_) => {
                self.push(&join(base, key), "value is too large");
                None
            }
        }
    }

    /// Byte size: integer bytes or a string such as `"256MB"` / `"32MiB"`.
    pub fn size(&mut self, t: &mut Table, base: &str, key: &str, allow_zero: bool) -> Option<u64> {
        let path = join(base, key);
        let parsed = match t.remove(key)? {
            Value::Integer(i) if i >= 0 => Some(i as u64),
            Value::String(s) => parse_size(&s),
            _ => None,
        };
        match parsed {
            Some(0) if !allow_zero => {
                self.push(&path, "must be greater than zero");
                None
            }
            Some(v) => Some(v),
            None => {
                self.push(
                    &path,
                    "must be a size such as \"256MB\", \"32MiB\", \"512KB\" or a byte count (MB = MiB, KB = KiB)",
                );
                None
            }
        }
    }

    /// Duration in seconds: integer seconds or a string such as `"10m"`; `0` only with
    /// `allow_zero`.
    pub fn duration_secs(
        &mut self,
        t: &mut Table,
        base: &str,
        key: &str,
        allow_zero: bool,
    ) -> Option<u64> {
        let path = join(base, key);
        let parsed = match t.remove(key)? {
            Value::Integer(i) if i >= 0 => Some(i as u64),
            Value::String(s) => parse_duration(&s),
            _ => None,
        };
        match parsed {
            Some(0) if !allow_zero => None,
            other => other,
        }
        .or_else(|| {
            let what = if allow_zero { "a" } else { "a positive" };
            self.push(
                &path,
                format!(
                    "must be {what} duration such as \"10m\", \"90s\", \"2h\" or a number of seconds"
                ),
            );
            None
        })
    }

    /// A secret that may be given inline (`key`) or as a file path (`key_file`).
    /// Setting both is a problem; the value is never echoed.
    pub fn secret(&mut self, t: &mut Table, base: &str, key: &str) -> Option<Secret> {
        self.secret_string(t, base, key).map(Secret::new)
    }

    pub fn secret_string(&mut self, t: &mut Table, base: &str, key: &str) -> Option<String> {
        let file_key = format!("{key}_file");
        let inline = self.string(t, base, key);
        let file = self.string(t, base, &file_key);
        match (inline, file) {
            (Some(_), Some(_)) => {
                self.push(
                    &join(base, key),
                    format!("set both `{key}` and `{file_key}`; use only one"),
                );
                None
            }
            (Some(v), None) => Some(v),
            (None, Some(file)) => {
                let path = join(base, &file_key);
                let full = resolve(&self.data_dir, &file);
                match std::fs::read_to_string(&full) {
                    Ok(content) => {
                        let value = content
                            .strip_suffix("\r\n")
                            .or_else(|| content.strip_suffix('\n'))
                            .unwrap_or(&content);
                        if value.is_empty() {
                            self.push(&path, format!("secret file {file} is empty"));
                            None
                        } else {
                            Some(value.to_owned())
                        }
                    }
                    Err(e) => {
                        self.push(
                            &path,
                            format!("cannot read secret file {file}: {}", e.kind()),
                        );
                        None
                    }
                }
            }
            (None, None) => None,
        }
    }
}

pub(crate) fn resolve(data_dir: &Path, p: &str) -> PathBuf {
    let p = Path::new(p);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        data_dir.join(p)
    }
}

/// `"256MB"`, `"32MiB"`, `"512KB"`, `"4KiB"`, `"1GB"`, `"1GiB"`, `"100"` or `"100B"`;
/// MB = MiB = 1024², KB = KiB = 1024, GB = GiB = 1024³.
pub(crate) fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (digits, unit) = s.split_at(split);
    let n: u64 = digits.parse().ok()?;
    let mult: u64 = match unit.trim() {
        "" | "B" => 1,
        "KB" | "KiB" => 1 << 10,
        "MB" | "MiB" => 1 << 20,
        "GB" | "GiB" => 1 << 30,
        _ => return None,
    };
    n.checked_mul(mult)
}

fn parse_duration(s: &str) -> Option<u64> {
    let s = s.trim();
    let (digits, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: u64 = digits.parse().ok()?;
    let mult = match unit {
        "" | "s" => 1,
        "m" => 60,
        "h" => 3600,
        _ => return None,
    };
    n.checked_mul(mult)
}
