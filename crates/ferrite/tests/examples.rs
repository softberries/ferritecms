//! The `examples/cluster` compose example, checked without Docker (P24): the shared
//! `ferrite.toml` plus `ferrite.env` must load as a valid cluster configuration, and
//! every `FERRITE_*` variable the compose file sets must be a known setting. CI also
//! runs `docker compose config` and `docker build`; nobody runs `docker compose up`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use assert_cmd::cargo::cargo_bin_cmd;
use ferrite_config::{ClusterMode, DbKind, MediaSection};

fn example_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/cluster")
}

/// `KEY=VALUE` lines; blank lines and `#` comments skipped (docker's env_file format).
fn parse_env_file(src: &str) -> Vec<(String, String)> {
    src.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let (k, v) = l.split_once('=').expect("KEY=VALUE line");
            (k.trim().to_owned(), v.trim().to_owned())
        })
        .collect()
}

/// Every distinct `FERRITE_[A-Z0-9_]+` name in `src`, ignoring YAML comments (whole lines
/// and trailing ones).
fn ferrite_names(src: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in src.lines() {
        let mut rest = strip_comment(line);
        while let Some(i) = rest.find("FERRITE_") {
            let tail = &rest[i..];
            let end = tail
                .find(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'))
                .unwrap_or(tail.len());
            let name = &tail[..end];
            if !names.iter().any(|n| n == name) {
                names.push(name.to_owned());
            }
            rest = &tail[end..];
        }
    }
    names
}

/// `line` without its comment: from a `#` at the start or after whitespace (the YAML and
/// env-file rule; a `#` inside a word, as in `a#b`, is kept).
fn strip_comment(line: &str) -> &str {
    let mut prev_space = true;
    for (i, c) in line.char_indices() {
        if c == '#' && prev_space {
            return &line[..i];
        }
        prev_space = c.is_whitespace();
    }
    line
}

/// A temp data folder holding a copy of the example's `ferrite.toml`.
fn data_dir() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::copy(
        example_dir().join("ferrite.toml"),
        tmp.path().join("ferrite.toml"),
    )
    .unwrap();
    tmp
}

#[test]
fn cluster_example_config_is_valid() {
    let env = parse_env_file(&std::fs::read_to_string(example_dir().join("ferrite.env")).unwrap());
    let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(
        keys,
        [
            "FERRITE_DATABASE__URL",
            "FERRITE_MEDIA__S3__ACCESS_KEY_ID",
            "FERRITE_MEDIA__S3__SECRET_ACCESS_KEY"
        ]
    );
    let data = data_dir();
    let config = ferrite_config::load(data.path(), env.clone()).expect("example config loads");

    assert_eq!(config.effective_cluster_mode(), ClusterMode::Cluster);
    assert_eq!(config.cluster.mode, ClusterMode::Cluster);
    assert_eq!(config.site.url.as_str(), "http://localhost:8080/");
    assert_eq!(config.server.listen.to_string(), "0.0.0.0:8080");
    assert_eq!(
        config
            .server
            .trusted_proxies
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        ["172.28.0.0/16"]
    );
    match &config.media {
        MediaSection::S3 {
            bucket,
            region,
            endpoint,
            allow_http,
            access_key_id,
            secret_access_key,
            ..
        } => {
            assert_eq!(bucket, "ferrite");
            assert_eq!(region, "us-east-1");
            assert_eq!(endpoint.as_deref(), Some("http://minio:9000"));
            assert!(allow_http);
            assert!(access_key_id.is_some() && secret_access_key.is_some());
        }
        other => panic!("expected S3 media, got {other:?}"),
    }
    match config.database_kind() {
        DbKind::Postgres { url } => {
            let url = url::Url::parse(url.expose()).unwrap();
            assert_eq!(url.host_str(), Some("postgres"));
        }
        other => panic!("expected Postgres, got {other:?}"),
    }

    // The compose subnet must be the one trusted_proxies names.
    let compose = std::fs::read_to_string(example_dir().join("docker-compose.yml")).unwrap();
    assert!(
        compose.contains("subnet: 172.28.0.0/16"),
        "compose subnet changed"
    );

    // `ferrite config check` (what an operator runs) agrees.
    let out = cargo_bin_cmd!("ferrite")
        .env_clear()
        .envs(env)
        .args(["config", "check", "--data"])
        .arg(data.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8(out).unwrap();
    assert!(
        out.starts_with("ok: mode=cluster database=postgres media=s3 workers="),
        "{out}"
    );
}

#[test]
fn compose_ferrite_variables_are_known_settings() {
    let env = parse_env_file(&std::fs::read_to_string(example_dir().join("ferrite.env")).unwrap());
    let compose = std::fs::read_to_string(example_dir().join("docker-compose.yml")).unwrap();
    let names = ferrite_names(&compose);
    assert!(
        names.iter().any(|n| n == "FERRITE_CLUSTER__NODE_NAME"),
        "{names:?}"
    );
    let data = data_dir();
    for name in &names {
        let mut vars = env.clone();
        vars.push((name.clone(), "dummy".to_owned()));
        if let Err(e) = ferrite_config::load(data.path(), vars) {
            panic!("{name} from docker-compose.yml is not a valid setting: {e}");
        }
    }
}

#[test]
fn ferrite_names_ignores_comment_lines() {
    let src =
        "# FERRITE_NOT_A_KEY in a comment\n  # indented FERRITE_ALSO_NOT\n  FERRITE_A__B: x\n";
    assert_eq!(ferrite_names(src), ["FERRITE_A__B"]);
}

#[test]
fn ferrite_names_ignores_trailing_comments() {
    let src = "  FERRITE_A__B: x # was FERRITE_OLD__KEY\nFERRITE_C__D=1\t# FERRITE_GONE\n\
               FERRITE_E__F: a#FERRITE_KEPT\n";
    assert_eq!(
        ferrite_names(src),
        [
            "FERRITE_A__B",
            "FERRITE_C__D",
            "FERRITE_E__F",
            "FERRITE_KEPT"
        ]
    );
}
