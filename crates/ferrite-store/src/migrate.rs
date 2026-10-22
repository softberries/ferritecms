use semver::Version;

use crate::StoreError;

/// `schema_meta` key holding the minimum binary version the schema requires.
pub const SCHEMA_META_MIN_BINARY: &str = "min_binary_version";

/// Migration phase (expand/contract, 02 §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Expand,
    Contract,
}

/// Parsed `-- ferrite:` header directives of an `.up.sql`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directives {
    pub phase: Phase,
    pub min_binary: Option<Version>,
    pub lossy_down: bool,
}

/// Parses the leading `-- ferrite:` directive lines; stops at the first SQL line.
pub fn parse_directives(up_sql: &str) -> Result<Directives, StoreError> {
    let mut d = Directives {
        phase: Phase::Expand,
        min_binary: None,
        lossy_down: false,
    };
    for line in up_sql.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some(comment) = line.strip_prefix("--") else {
            break;
        };
        let Some(body) = comment.trim().strip_prefix("ferrite:") else {
            continue;
        };
        // Strip a trailing explanatory comment.
        let body = body.split("--").next().unwrap_or("").trim();
        let mut parts = body.split_whitespace();
        let name = parts.next().unwrap_or("");
        let arg = parts.next();
        if parts.next().is_some() {
            return Err(directive_err(format!("too many arguments in `{body}`")));
        }
        match (name, arg) {
            ("phase", Some("expand")) => d.phase = Phase::Expand,
            ("phase", Some("contract")) => d.phase = Phase::Contract,
            ("phase", other) => {
                return Err(directive_err(format!("invalid phase {other:?}")));
            }
            ("min-binary", Some(v)) => {
                let v = Version::parse(v)
                    .map_err(|e| directive_err(format!("invalid min-binary `{v}`: {e}")))?;
                d.min_binary = Some(v);
            }
            ("min-binary", None) => return Err(directive_err("min-binary needs a version".into())),
            ("lossy-down", None) => d.lossy_down = true,
            ("lossy-down", Some(_)) => {
                return Err(directive_err("lossy-down takes no argument".into()));
            }
            (other, _) => return Err(directive_err(format!("unknown directive `{other}`"))),
        }
    }
    if d.min_binary.is_some() && d.phase != Phase::Contract {
        return Err(directive_err(
            "min-binary is only allowed on contract migrations".into(),
        ));
    }
    Ok(d)
}

fn directive_err(msg: String) -> StoreError {
    StoreError::Migration(format!("bad ferrite directive: {msg}"))
}

/// Returns problems found in an expand migration (02 §8 lint over SQL text).
pub fn expand_lint(up_sql: &str) -> Vec<String> {
    let stripped: String = up_sql
        .lines()
        .map(|l| l.split("--").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
        .to_uppercase();
    let mut problems = Vec::new();
    for stmt in stripped.split(';') {
        let words = words(stmt);
        if words.is_empty() {
            continue;
        }
        let label = words.iter().take(3).copied().collect::<Vec<_>>().join(" ");
        if words.contains(&"DROP") {
            problems.push(format!("DROP in expand migration: `{label} ...`"));
        }
        if words.contains(&"RENAME") {
            problems.push(format!("RENAME in expand migration: `{label} ...`"));
        }
        if words[0] == "ALTER" {
            if alters_type(&words) {
                problems.push(format!("ALTER ... TYPE in expand migration: `{label} ...`"));
            }
            for clause in split_clauses(stmt) {
                let w = self::words(&clause);
                let adds = w.contains(&"ADD");
                if adds && w.windows(2).any(|p| p == ["NOT", "NULL"]) && !w.contains(&"DEFAULT") {
                    problems.push(format!(
                        "ADD COLUMN NOT NULL without DEFAULT: `{}`",
                        clause.split_whitespace().collect::<Vec<_>>().join(" ")
                    ));
                }
            }
        }
    }
    problems
}

fn words(s: &str) -> Vec<&str> {
    s.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .collect()
}

/// Detects `ALTER [COLUMN] name [SET DATA] TYPE`.
fn alters_type(words: &[&str]) -> bool {
    words.iter().enumerate().any(|(i, w)| {
        if *w != "ALTER" || i == 0 {
            return false;
        }
        let mut j = i + 1;
        if words.get(j) == Some(&"COLUMN") {
            j += 1;
        }
        j += 1; // column name
        if words.get(j) == Some(&"SET") && words.get(j + 1) == Some(&"DATA") {
            j += 2;
        }
        words.get(j) == Some(&"TYPE")
    })
}

/// Splits a statement on top-level commas (outside parentheses).
fn split_clauses(stmt: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for c in stmt.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    out.push(cur);
    out
}

/// Result of comparing the database schema with this binary (02 §8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Compat {
    UpToDate,
    NeedsUp,
    NewerSchemaOk,
    Incompatible { requires: Version },
}

/// Migration state of a database relative to this binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationStatus {
    pub db_version: i64,
    pub binary_version: i64,
    pub min_binary: Option<Version>,
    pub pending: Vec<i64>,
    pub unknown_applied: Vec<i64>,
    pub verdict: Compat,
}

/// Startup compatibility check, exactly the 02 §8 table.
pub fn verdict(db: i64, bin: i64, binary: &Version, min_binary: Option<&Version>) -> Compat {
    if db == bin {
        Compat::UpToDate
    } else if db < bin {
        Compat::NeedsUp
    } else {
        match min_binary {
            Some(min) if binary < min => Compat::Incompatible {
                requires: min.clone(),
            },
            _ => Compat::NewerSchemaOk,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn verdict_table() {
        let b = v("0.4.0");
        assert_eq!(verdict(3, 3, &b, None), Compat::UpToDate);
        assert_eq!(verdict(3, 3, &b, Some(&v("9.0.0"))), Compat::UpToDate);
        assert_eq!(verdict(2, 3, &b, None), Compat::NeedsUp);
        assert_eq!(verdict(5, 3, &b, None), Compat::NewerSchemaOk);
        assert_eq!(verdict(5, 3, &b, Some(&v("0.4.0"))), Compat::NewerSchemaOk);
        assert_eq!(verdict(5, 3, &b, Some(&v("0.3.0"))), Compat::NewerSchemaOk);
        assert_eq!(
            verdict(5, 3, &b, Some(&v("0.5.0"))),
            Compat::Incompatible {
                requires: v("0.5.0")
            }
        );
    }

    #[test]
    fn directives_parse() {
        let d = parse_directives("CREATE TABLE t (a INT);").unwrap();
        assert_eq!(
            d,
            Directives {
                phase: Phase::Expand,
                min_binary: None,
                lossy_down: false
            }
        );
        let d = parse_directives(
            "-- ferrite:phase contract\n-- ferrite:min-binary 0.4.0   -- why\n\n-- note\n-- ferrite:lossy-down\nDROP TABLE x;",
        )
        .unwrap();
        assert_eq!(d.phase, Phase::Contract);
        assert_eq!(d.min_binary, Some(v("0.4.0")));
        assert!(d.lossy_down);
        assert!(parse_directives("-- ferrite:bogus\nSELECT 1;").is_err());
        assert!(parse_directives("-- ferrite:phase sideways\nSELECT 1;").is_err());
        assert!(parse_directives("-- ferrite:min-binary 0.4.0\nSELECT 1;").is_err());
        assert!(
            parse_directives("-- ferrite:min-binary nope\n-- ferrite:phase contract\n").is_err()
        );
        let d = parse_directives("SELECT 1;\n-- ferrite:phase contract\n").unwrap();
        assert_eq!(d.phase, Phase::Expand);
    }

    #[test]
    fn expand_lint_flags() {
        assert!(!expand_lint("DROP TABLE x;").is_empty());
        assert!(!expand_lint("drop index i;").is_empty());
        assert!(!expand_lint("ALTER TABLE t ADD COLUMN c TEXT NOT NULL;").is_empty());
        assert!(expand_lint("ALTER TABLE t ADD COLUMN c TEXT NOT NULL DEFAULT '';").is_empty());
        assert!(expand_lint("ALTER TABLE t ADD COLUMN c TEXT;").is_empty());
        assert!(expand_lint("CREATE TABLE t (a TEXT NOT NULL);").is_empty());
        assert!(expand_lint("-- DROP in a comment\nCREATE TABLE t (a INT);").is_empty());
        assert!(!expand_lint("ALTER TABLE t RENAME COLUMN a TO b;").is_empty());
        assert!(!expand_lint("ALTER TABLE t ALTER COLUMN c TYPE int;").is_empty());
        assert!(!expand_lint("ALTER TABLE t ALTER COLUMN c SET DATA TYPE int;").is_empty());
        assert!(
            !expand_lint("ALTER TABLE t ADD COLUMN a INT DEFAULT 0, ADD COLUMN b INT NOT NULL;")
                .is_empty()
        );
        assert!(expand_lint("ALTER TABLE t ADD COLUMN type TEXT;").is_empty());
    }
}
