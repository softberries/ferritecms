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

impl MigrationStatus {
    /// Builds the status from applied and embedded versions plus `schema_meta.min_binary_version`.
    pub fn compute(
        applied: &[i64],
        embedded: &[i64],
        binary: &Version,
        min_binary: Option<Version>,
    ) -> Self {
        let db_version = applied.iter().copied().max().unwrap_or(0);
        let binary_version = embedded.iter().copied().max().unwrap_or(0);
        let pending: Vec<i64> = embedded
            .iter()
            .filter(|v| !applied.contains(v))
            .copied()
            .collect();
        let unknown_applied = applied
            .iter()
            .filter(|v| !embedded.contains(v))
            .copied()
            .collect();
        let mut verdict = verdict(db_version, binary_version, binary, min_binary.as_ref());
        // A gap (an older embedded migration not applied) still needs `up`.
        if verdict == Compat::UpToDate && !pending.is_empty() {
            verdict = Compat::NeedsUp;
        }
        Self {
            db_version,
            binary_version,
            min_binary,
            pending,
            unknown_applied,
            verdict,
        }
    }
}

/// Versions to revert (newest first) to reach `to`. `embedded_up` pairs each embedded
/// version with its `.up.sql`. Refuses versions this binary does not know and, unless
/// `allow_lossy`, migrations marked `-- ferrite:lossy-down`.
pub fn plan_down(
    applied: &[i64],
    embedded_up: &[(i64, &str)],
    to: i64,
    allow_lossy: bool,
) -> Result<Vec<i64>, StoreError> {
    let mut steps: Vec<i64> = applied.iter().copied().filter(|v| *v > to).collect();
    steps.sort_unstable_by(|a, b| b.cmp(a));
    for v in &steps {
        let Some((_, sql)) = embedded_up.iter().find(|(e, _)| e == v) else {
            return Err(StoreError::Migration(format!(
                "migration {v} is not known to this binary; run `down` with the binary that applied it"
            )));
        };
        if parse_directives(sql)?.lossy_down && !allow_lossy {
            return Err(StoreError::Migration(format!(
                "migration {v} has a lossy down (data removed by its up cannot be restored); pass allow_lossy to proceed"
            )));
        }
    }
    Ok(steps)
}

/// Highest `-- ferrite:min-binary` among the applied, embedded migrations.
pub fn required_min_binary(
    applied: &[i64],
    embedded_up: &[(i64, &str)],
) -> Result<Option<Version>, StoreError> {
    let mut max: Option<Version> = None;
    for (v, sql) in embedded_up {
        if !applied.contains(v) {
            continue;
        }
        if let Some(min) = parse_directives(sql)?.min_binary
            && max.as_ref().is_none_or(|m| &min > m)
        {
            max = Some(min);
        }
    }
    Ok(max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_compute() {
        let b = v("0.1.0");
        let s = MigrationStatus::compute(&[1, 2], &[1, 2], &b, None);
        assert_eq!((s.db_version, s.binary_version), (2, 2));
        assert_eq!(s.verdict, Compat::UpToDate);
        assert!(s.pending.is_empty() && s.unknown_applied.is_empty());

        let s = MigrationStatus::compute(&[], &[1, 2], &b, None);
        assert_eq!((s.db_version, s.pending.clone()), (0, vec![1, 2]));
        assert_eq!(s.verdict, Compat::NeedsUp);

        let s = MigrationStatus::compute(&[1, 2, 3], &[1, 2], &b, Some(v("0.2.0")));
        assert_eq!(s.unknown_applied, vec![3]);
        assert_eq!(
            s.verdict,
            Compat::Incompatible {
                requires: v("0.2.0")
            }
        );

        // Gap: db == bin but an older migration is missing.
        let s = MigrationStatus::compute(&[1, 3], &[1, 2, 3], &b, None);
        assert_eq!(s.pending, vec![2]);
        assert_eq!(s.verdict, Compat::NeedsUp);
    }

    #[test]
    fn plan_down_rules() {
        let lossy = "-- ferrite:phase contract\n-- ferrite:lossy-down\nDROP TABLE x;";
        let emb = [(1, "CREATE TABLE a (x INT);"), (2, lossy), (3, "SELECT 1;")];
        assert_eq!(plan_down(&[1, 2, 3], &emb, 2, false).unwrap(), vec![3]);
        assert_eq!(
            plan_down(&[1, 2, 3], &emb, 3, false).unwrap(),
            Vec::<i64>::new()
        );
        let err = plan_down(&[1, 2, 3], &emb, 0, false).unwrap_err();
        assert!(matches!(&err, StoreError::Migration(m) if m.contains("migration 2")));
        assert_eq!(plan_down(&[1, 2, 3], &emb, 0, true).unwrap(), vec![3, 2, 1]);
        let err = plan_down(&[1, 4], &emb, 0, true).unwrap_err();
        assert!(matches!(&err, StoreError::Migration(m) if m.contains("migration 4")));
    }

    #[test]
    fn required_min_binary_takes_max_of_applied() {
        let c =
            |m: &str| format!("-- ferrite:phase contract\n-- ferrite:min-binary {m}\nSELECT 1;");
        let (a, b) = (c("0.3.0"), c("0.2.0"));
        let emb = [(1, "SELECT 1;"), (2, a.as_str()), (3, b.as_str())];
        assert_eq!(required_min_binary(&[1], &emb).unwrap(), None);
        assert_eq!(
            required_min_binary(&[1, 2, 3], &emb).unwrap(),
            Some(v("0.3.0"))
        );
        assert_eq!(
            required_min_binary(&[1, 3], &emb).unwrap(),
            Some(v("0.2.0"))
        );
    }

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
