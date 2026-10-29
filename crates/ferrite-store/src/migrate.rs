use semver::Version;

use crate::StoreError;

/// `schema_meta` key holding the minimum binary version the schema requires.
pub const SCHEMA_META_MIN_BINARY: &str = "min_binary_version";

/// `schema_meta` key holding this database's install id (M4-1 R16): a UUID written by the
/// migrations when absent and never exported (unlike `site_id`, which every copy of a site
/// shares). `ferrite restore` gives a restored database a new one; the artifacts GC deletes
/// from a media store only while the store's owner marker names it.
pub const SCHEMA_META_INSTALL_ID: &str = "install_id";

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

/// Returns problems found in an expand migration (02 §8 lint over SQL text). `down_sql`
/// is the migration's `.down.sql`: a CHECK swapped in place passes only when it widens
/// the one the down restores.
pub fn expand_lint(up_sql: &str, down_sql: &str) -> Vec<String> {
    let down = strip_comments(down_sql);
    let mut problems = Vec::new();
    for original in strip_comments(up_sql).split(';') {
        let stmt = original.to_uppercase();
        let stmt = stmt.as_str();
        let words = words(stmt);
        if words.is_empty() {
            continue;
        }
        let label = words.iter().take(3).copied().collect::<Vec<_>>().join(" ");
        if words.contains(&"DROP") && !widens_checks(original, &down) {
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

fn strip_comments(sql: &str) -> String {
    sql.lines()
        .map(|l| l.split("--").next().unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `ALTER TABLE t DROP CONSTRAINT c, ADD CONSTRAINT c CHECK (col IN ('a', 'b', …))`
/// (how a Postgres CHECK is widened, `0010`): every clause is either the table prefix, a
/// `DROP CONSTRAINT` or an `ADD CONSTRAINT`, and each dropped constraint comes back under
/// its name as a `col IN (<string literals>)` CHECK whose list is a strict superset of
/// the one the down's `ADD CONSTRAINT` of that name restores, on the same column.
/// Anything else (narrowing, an equal list, another constraint type or column, a down
/// that does not restore such a CHECK) is not a widening.
fn widens_checks(stmt: &str, down: &str) -> bool {
    let Some(swap) = constraint_clauses(stmt) else {
        return false;
    };
    let dropped: Vec<&String> = swap
        .iter()
        .filter_map(|c| match c {
            ConstraintClause::Drop(name) => Some(name),
            ConstraintClause::Add(..) => None,
        })
        .collect();
    let added = |clauses: &[ConstraintClause], name: &str| {
        clauses.iter().rev().find_map(|c| match c {
            ConstraintClause::Add(n, check) if n == name => Some(check.clone()),
            _ => None,
        })
    };
    let restored = |name: &str| {
        down.rsplit(';')
            .filter_map(constraint_clauses)
            .find_map(|clauses| added(&clauses, name))
            .flatten()
    };
    !dropped.is_empty()
        && dropped.iter().all(|name| {
            let (Some(Some(new)), Some(old)) = (added(&swap, name), restored(name)) else {
                return false;
            };
            new.column == old.column
                && new.values.len() > old.values.len()
                && old.values.iter().all(|v| new.values.contains(v))
        })
}

/// A `CHECK (column IN ('…', …))`; `column` upper-cased (an unquoted identifier).
#[derive(Debug, Clone)]
struct CheckIn {
    column: String,
    values: Vec<String>,
}

enum ConstraintClause {
    Drop(String),
    /// The constraint's name and, when it is one, its `CHECK (col IN (…))`.
    Add(String, Option<CheckIn>),
}

#[derive(Debug, PartialEq)]
enum Token {
    /// An identifier or keyword, upper-cased.
    Word(String),
    /// A string literal's value (`''` unescaped).
    Str(String),
    Punct(char),
}

fn tokens(sql: &str) -> Option<Vec<Token>> {
    let mut out = Vec::new();
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            continue;
        }
        if c.is_alphanumeric() || c == '_' {
            let mut word = c.to_uppercase().to_string();
            while let Some(&n) = chars.peek().filter(|n| n.is_alphanumeric() || **n == '_') {
                word.extend(n.to_uppercase());
                chars.next();
            }
            out.push(Token::Word(word));
        } else if c == '\'' {
            let mut text = String::new();
            loop {
                match chars.next()? {
                    '\'' if chars.peek() == Some(&'\'') => {
                        chars.next();
                        text.push('\'');
                    }
                    '\'' => break,
                    other => text.push(other),
                }
            }
            out.push(Token::Str(text));
        } else {
            out.push(Token::Punct(c));
        }
    }
    Some(out)
}

/// The clauses of an `ALTER TABLE` made only of `DROP CONSTRAINT name` and
/// `ADD CONSTRAINT name …` clauses; `None` for any other statement.
fn constraint_clauses(stmt: &str) -> Option<Vec<ConstraintClause>> {
    let tokens = tokens(stmt)?;
    let word = |t: &Token, w: &str| matches!(t, Token::Word(x) if x == w);
    if !(tokens.len() > 2 && word(&tokens[0], "ALTER") && word(&tokens[1], "TABLE")) {
        return None;
    }
    let mut clauses = Vec::new();
    let mut depth = 0i32;
    let mut start = 0;
    for (i, t) in tokens.iter().enumerate() {
        match t {
            Token::Punct('(') => depth += 1,
            Token::Punct(')') => depth -= 1,
            Token::Punct(',') if depth == 0 => {
                clauses.push(&tokens[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    clauses.push(&tokens[start..]);
    let mut out = Vec::new();
    for (n, clause) in clauses.into_iter().enumerate() {
        let at = clause
            .iter()
            .position(|t| word(t, "ADD") || word(t, "DROP"))?;
        // Only the first clause carries the `ALTER TABLE <name>` prefix.
        if (n == 0) != (at > 0) {
            return None;
        }
        match &clause[at..] {
            [Token::Word(op), c, Token::Word(name)] if op == "DROP" && word(c, "CONSTRAINT") => {
                out.push(ConstraintClause::Drop(name.clone()));
            }
            [Token::Word(op), c, Token::Word(name), rest @ ..]
                if op == "ADD" && word(c, "CONSTRAINT") =>
            {
                out.push(ConstraintClause::Add(name.clone(), check_in(rest)));
            }
            _ => return None,
        }
    }
    Some(out)
}

/// `CHECK ( col IN ( 'a' [, 'b' …] ) )`, exactly.
fn check_in(tokens: &[Token]) -> Option<CheckIn> {
    let [
        Token::Word(check),
        Token::Punct('('),
        Token::Word(column),
        Token::Word(op),
        Token::Punct('('),
        list @ ..,
        Token::Punct(')'),
        Token::Punct(')'),
    ] = tokens
    else {
        return None;
    };
    if check != "CHECK" || op != "IN" {
        return None;
    }
    let mut values = Vec::new();
    for (i, t) in list.iter().enumerate() {
        match (i % 2, t) {
            (0, Token::Str(v)) => values.push(v.clone()),
            (1, Token::Punct(',')) => {}
            _ => return None,
        }
    }
    (list.len() % 2 == 1).then_some(CheckIn {
        column: column.clone(),
        values,
    })
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

/// An embedded migration not yet applied, with its parsed header directives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingMigration {
    pub version: i64,
    pub description: String,
    pub directives: Directives,
}

/// Migration state of a database relative to this binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationStatus {
    pub db_version: i64,
    pub binary_version: i64,
    pub min_binary: Option<Version>,
    /// Embedded migrations not applied, oldest first.
    pub pending: Vec<PendingMigration>,
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
    /// Builds the status from applied versions and every embedded migration plus
    /// `schema_meta.min_binary_version`.
    pub fn compute(
        applied: &[i64],
        embedded: &[PendingMigration],
        binary: &Version,
        min_binary: Option<Version>,
    ) -> Self {
        let db_version = applied.iter().copied().max().unwrap_or(0);
        let binary_version = embedded.iter().map(|m| m.version).max().unwrap_or(0);
        let pending: Vec<PendingMigration> = embedded
            .iter()
            .filter(|m| !applied.contains(&m.version))
            .cloned()
            .collect();
        let unknown_applied = applied
            .iter()
            .filter(|v| !embedded.iter().any(|m| m.version == **v))
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

/// Versions to revert (newest first) to reach `to`. `embedded` pairs each embedded
/// version with its parsed directives. Refuses versions this binary does not know and,
/// unless `allow_lossy`, migrations marked `-- ferrite:lossy-down`.
pub fn plan_down(
    applied: &[i64],
    embedded: &[(i64, Directives)],
    to: i64,
    allow_lossy: bool,
) -> Result<Vec<i64>, StoreError> {
    let mut steps: Vec<i64> = applied.iter().copied().filter(|v| *v > to).collect();
    steps.sort_unstable_by(|a, b| b.cmp(a));
    for v in &steps {
        let Some((_, d)) = embedded.iter().find(|(e, _)| e == v) else {
            return Err(StoreError::Migration(format!(
                "migration {v} is not known to this binary; run `down` with the binary that applied it"
            )));
        };
        if d.lossy_down && !allow_lossy {
            return Err(StoreError::Migration(format!(
                "migration {v} has a lossy down (data removed by its up cannot be restored); pass allow_lossy to proceed"
            )));
        }
    }
    Ok(steps)
}

/// The `schema_meta.min_binary_version` to store after this binary migrated.
///
/// - Raised to the highest `min-binary` among applied, embedded contract migrations.
/// - Never lowered or erased while any applied version is unknown to this binary
///   (a newer binary's contract migration may have set it).
/// - Lowered or dropped only when this binary just reverted (`reverted`) the
///   migration whose `min-binary` equals the stored value.
pub fn next_min_binary(
    current: Option<&Version>,
    applied: &[i64],
    embedded: &[(i64, Directives)],
    reverted: &[i64],
) -> Option<Version> {
    let known = |v: &i64| embedded.iter().any(|(e, _)| e == v);
    let from_applied = embedded
        .iter()
        .filter(|(v, _)| applied.contains(v))
        .filter_map(|(_, d)| d.min_binary.as_ref())
        .max();
    let reverted_setter = embedded.iter().any(|(v, d)| {
        reverted.contains(v) && current.is_some() && d.min_binary.as_ref() == current
    });
    let keep_current = applied.iter().any(|v| !known(v)) || !reverted_setter;
    let floor = if keep_current { current } else { None };
    floor.max(from_applied).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embedded(versions: &[i64]) -> Vec<PendingMigration> {
        versions
            .iter()
            .map(|&version| PendingMigration {
                version,
                description: format!("m{version}"),
                directives: d("SELECT 1;"),
            })
            .collect()
    }

    fn pending_versions(s: &MigrationStatus) -> Vec<i64> {
        s.pending.iter().map(|p| p.version).collect()
    }

    #[test]
    fn status_compute() {
        let b = v("0.1.0");
        let s = MigrationStatus::compute(&[1, 2], &embedded(&[1, 2]), &b, None);
        assert_eq!((s.db_version, s.binary_version), (2, 2));
        assert_eq!(s.verdict, Compat::UpToDate);
        assert!(s.pending.is_empty() && s.unknown_applied.is_empty());

        let s = MigrationStatus::compute(&[], &embedded(&[1, 2]), &b, None);
        assert_eq!((s.db_version, pending_versions(&s)), (0, vec![1, 2]));
        assert_eq!(s.verdict, Compat::NeedsUp);

        let s = MigrationStatus::compute(&[1, 2, 3], &embedded(&[1, 2]), &b, Some(v("0.2.0")));
        assert_eq!(s.unknown_applied, vec![3]);
        assert_eq!(
            s.verdict,
            Compat::Incompatible {
                requires: v("0.2.0")
            }
        );

        // Gap: db == bin but an older migration is missing.
        let s = MigrationStatus::compute(&[1, 3], &embedded(&[1, 2, 3]), &b, None);
        assert_eq!(pending_versions(&s), vec![2]);
        assert_eq!(s.verdict, Compat::NeedsUp);
    }

    fn d(sql: &str) -> Directives {
        parse_directives(sql).unwrap()
    }

    fn contract(min: &str) -> Directives {
        d(&format!(
            "-- ferrite:phase contract\n-- ferrite:min-binary {min}\nSELECT 1;"
        ))
    }

    #[test]
    fn plan_down_rules() {
        let lossy = d("-- ferrite:phase contract\n-- ferrite:lossy-down\nDROP TABLE x;");
        let plain = d("SELECT 1;");
        let emb = [(1, plain.clone()), (2, lossy), (3, plain)];
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
    fn next_min_binary_rules() {
        let emb = [
            (1, d("SELECT 1;")),
            (2, contract("0.3.0")),
            (3, contract("0.2.0")),
        ];
        let (v2, v3, v9) = (v("0.2.0"), v("0.3.0"), v("0.9.0"));
        // Nothing contract applied, nothing stored.
        assert_eq!(next_min_binary(None, &[1], &emb, &[]), None);
        // Raised by applied contract migrations (max wins).
        assert_eq!(
            next_min_binary(None, &[1, 2, 3], &emb, &[]),
            Some(v3.clone())
        );
        assert_eq!(
            next_min_binary(Some(&v2), &[1, 2, 3], &emb, &[]),
            Some(v3.clone())
        );
        // Older binary (knows only 1) against a newer schema: never erased or lowered.
        let old = &emb[..1];
        assert_eq!(
            next_min_binary(Some(&v9), &[1, 2, 7], old, &[]),
            Some(v9.clone())
        );
        assert_eq!(
            next_min_binary(Some(&v9), &[1, 7], &emb, &[]),
            Some(v9.clone())
        );
        // A higher stored value is kept when nothing that set it was reverted.
        assert_eq!(
            next_min_binary(Some(&v9), &[1, 2], &emb, &[]),
            Some(v9.clone())
        );
        // Reverting the setter lowers to what remains, or drops it.
        assert_eq!(
            next_min_binary(Some(&v3), &[1, 3], &emb, &[2]),
            Some(v2.clone())
        );
        assert_eq!(next_min_binary(Some(&v3), &[1], &emb, &[3, 2]), None);
        // Reverting a migration that did not set the stored value changes nothing.
        assert_eq!(
            next_min_binary(Some(&v3), &[1, 2], &emb, &[3]),
            Some(v3.clone())
        );
        // Reverted setter but an unknown version is still applied: keep.
        assert_eq!(next_min_binary(Some(&v3), &[1, 8], &emb, &[2]), Some(v3));
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
        let lint = |up: &str| expand_lint(up, "");
        assert!(!lint("DROP TABLE x;").is_empty());
        assert!(!lint("drop index i;").is_empty());
        assert!(!lint("ALTER TABLE t ADD COLUMN c TEXT NOT NULL;").is_empty());
        assert!(lint("ALTER TABLE t ADD COLUMN c TEXT NOT NULL DEFAULT '';").is_empty());
        assert!(lint("ALTER TABLE t ADD COLUMN c TEXT;").is_empty());
        assert!(lint("CREATE TABLE t (a TEXT NOT NULL);").is_empty());
        assert!(lint("-- DROP in a comment\nCREATE TABLE t (a INT);").is_empty());
        assert!(!lint("ALTER TABLE t RENAME COLUMN a TO b;").is_empty());
        assert!(!lint("ALTER TABLE t ALTER COLUMN c TYPE int;").is_empty());
        assert!(!lint("ALTER TABLE t ALTER COLUMN c SET DATA TYPE int;").is_empty());
        assert!(
            !lint("ALTER TABLE t ADD COLUMN a INT DEFAULT 0, ADD COLUMN b INT NOT NULL;")
                .is_empty()
        );
        assert!(lint("ALTER TABLE t ADD COLUMN type TEXT;").is_empty());
    }

    /// A CHECK swapped under its name in one statement passes only when it widens a
    /// `col IN ('…')` list: the up's list is a strict superset of the one the down
    /// restores, on the same column.
    #[test]
    fn expand_lint_constraint_swaps() {
        const SWAP: &str = "ALTER TABLE t DROP CONSTRAINT t_k_check, ";
        let down = "ALTER TABLE t DROP CONSTRAINT t_k_check, \
                    ADD CONSTRAINT t_k_check CHECK (k IN ('a', 'b'));";
        let up = |add: &str| format!("{SWAP}{add};");

        // Widening passes, case-insensitive keywords included.
        assert_eq!(
            expand_lint(
                &up("ADD CONSTRAINT t_k_check CHECK (k IN ('a', 'b', 'c'))"),
                down
            ),
            Vec::<String>::new()
        );
        assert!(
            expand_lint(
                "alter table t drop constraint t_k_check, \
                 add constraint t_k_check check (k in ('b', 'a', 'it''s'));",
                down
            )
            .is_empty()
        );
        // Narrowing, an equal list, another constraint type, another column.
        for add in [
            "ADD CONSTRAINT t_k_check CHECK (k IN ('a'))",
            "ADD CONSTRAINT t_k_check CHECK (k IN ('a', 'c'))",
            "ADD CONSTRAINT t_k_check CHECK (k IN ('b', 'a'))",
            "ADD CONSTRAINT t_k_check UNIQUE (k)",
            "ADD CONSTRAINT t_k_check CHECK (k <> 'x')",
            "ADD CONSTRAINT t_k_check CHECK (j IN ('a', 'b', 'c'))",
            "ADD CONSTRAINT t_k_check CHECK (k IN ('a', 'b', 'c') OR true)",
            "ADD CONSTRAINT other CHECK (k IN ('a', 'b', 'c'))",
        ] {
            assert!(!expand_lint(&up(add), down).is_empty(), "{add}");
        }
        // The down must restore an IN list on the same column.
        let wide = up("ADD CONSTRAINT t_k_check CHECK (k IN ('a', 'b', 'c'))");
        assert!(!expand_lint(&wide, "").is_empty());
        assert!(
            !expand_lint(
                &wide,
                "ALTER TABLE t DROP CONSTRAINT t_k_check, ADD CONSTRAINT t_k_check UNIQUE (k);"
            )
            .is_empty()
        );
        assert!(
            !expand_lint(
                &wide,
                "ALTER TABLE t DROP CONSTRAINT t_k_check, \
                 ADD CONSTRAINT t_k_check CHECK (j IN ('a'));"
            )
            .is_empty()
        );
        // A lone drop, or one beside another drop, is still a DROP.
        assert!(!expand_lint("ALTER TABLE t DROP CONSTRAINT t_k_check;", down).is_empty());
        assert!(
            !expand_lint(
                "ALTER TABLE t DROP CONSTRAINT t_k_check, DROP COLUMN x, \
                 ADD CONSTRAINT t_k_check CHECK (k IN ('a', 'b', 'c'));",
                down
            )
            .is_empty()
        );
        assert!(!expand_lint("DROP TABLE t, ADD CONSTRAINT t CHECK (true);", down).is_empty());
    }

    /// The real Postgres `0010_import_jobs` widens `artifacts_kind_check` and passes.
    #[test]
    fn expand_lint_passes_postgres_0010() {
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../ferrite-store-postgres/migrations"
        );
        let read = |f: &str| std::fs::read_to_string(format!("{dir}/{f}")).unwrap();
        let up = read("0010_import_jobs.up.sql");
        assert_eq!(
            expand_lint(&up, &read("0010_import_jobs.down.sql")),
            Vec::<String>::new()
        );
        // Without the down's restored list the swap cannot be proven wider.
        assert!(!expand_lint(&up, "").is_empty());
    }
}
