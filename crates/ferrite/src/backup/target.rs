//! Where a backup goes or a restore reads from (M4-1 P13–P14).

use std::path::{Path, PathBuf};

use anyhow::bail;

/// A `--to` of `ferrite backup` or the source of `ferrite restore`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A single `.tar.zst` archive (P7, `kind: "backup"`).
    File(PathBuf),
    /// `-`: the archive on stdout (backup) or stdin (restore).
    Stdout,
    /// A backup repository (P13).
    Repo(RepoLocation),
}

/// Where a repository lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoLocation {
    /// A folder (created by the first backup).
    Dir(PathBuf),
    /// `s3://bucket[/prefix]`, with credentials and endpoint from the environment.
    S3(String),
}

impl Target {
    /// `-` is [`Target::Stdout`]; `s3://…` a repository in a bucket; a path ending in
    /// `.tar.zst` an archive; any other path a repository folder. A relative path is
    /// relative to `base` (the working directory for the CLI, the data folder for the
    /// `[backup]` schedule).
    pub fn parse(s: &str, base: &Path) -> anyhow::Result<Target> {
        if s.is_empty() {
            bail!("expected a .tar.zst file, -, a folder or s3://bucket[/prefix]");
        }
        if s == "-" {
            return Ok(Target::Stdout);
        }
        if s.starts_with("s3://") {
            return Ok(Target::Repo(RepoLocation::S3(s.to_owned())));
        }
        if s.contains("://") {
            bail!("unsupported backup location {s:?} (use a path, -, or s3://bucket[/prefix])");
        }
        let path = base.join(s);
        if s.ends_with(".tar.zst") {
            Ok(Target::File(path))
        } else {
            Ok(Target::Repo(RepoLocation::Dir(path)))
        }
    }

    /// For messages.
    pub fn describe(&self) -> String {
        match self {
            Target::File(p) | Target::Repo(RepoLocation::Dir(p)) => p.display().to_string(),
            Target::Stdout => "-".to_owned(),
            Target::Repo(RepoLocation::S3(url)) => url.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses() {
        let base = Path::new("/data");
        assert_eq!(Target::parse("-", base).unwrap(), Target::Stdout);
        assert_eq!(
            Target::parse("b.tar.zst", base).unwrap(),
            Target::File(PathBuf::from("/data/b.tar.zst"))
        );
        assert_eq!(
            Target::parse("/x/b.tar.zst", base).unwrap(),
            Target::File(PathBuf::from("/x/b.tar.zst"))
        );
        assert_eq!(
            Target::parse("repo", base).unwrap(),
            Target::Repo(RepoLocation::Dir(PathBuf::from("/data/repo")))
        );
        assert_eq!(
            Target::parse("s3://b/p", base).unwrap(),
            Target::Repo(RepoLocation::S3("s3://b/p".into()))
        );
        assert!(Target::parse("", base).is_err());
        assert!(Target::parse("gs://b", base).is_err());
    }
}
