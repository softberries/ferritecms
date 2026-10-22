use std::fmt;

/// One validation problem. `path` is a dotted key (`database.max_connections`),
/// `env:FERRITE_…` for a problem caused by an environment variable, or
/// `ferrite.toml` for file-level problems. Messages never contain secret values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigProblem {
    pub path: String,
    pub message: String,
}

/// All problems found while loading the configuration.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub struct ConfigError {
    pub problems: Vec<ConfigProblem>,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid configuration ({} problem(s))",
            self.problems.len()
        )?;
        for p in &self.problems {
            write!(f, "\n  {}: {}", p.path, p.message)?;
        }
        Ok(())
    }
}
