use ferrite_domain::Secret;

use crate::*;

fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn run(
    toml: Option<&str>,
    vars: &[(&str, &str)],
) -> (tempfile::TempDir, Result<Config, ConfigError>) {
    let dir = tempfile::tempdir().unwrap();
    if let Some(t) = toml {
        std::fs::write(dir.path().join("ferrite.toml"), t).unwrap();
    }
    let r = load(dir.path(), env(vars));
    (dir, r)
}

fn paths(e: &ConfigError) -> Vec<&str> {
    e.problems.iter().map(|p| p.path.as_str()).collect()
}

const MIN: &str = "[site]\nurl = \"https://example.com\"\n";

#[test]
fn defaults_minimal() {
    let (dir, r) = run(Some(MIN), &[]);
    let c = r.unwrap();
    assert_eq!(c.site.url.as_str(), "https://example.com/");
    assert_eq!(c.site.title, "");
    assert_eq!(c.site.locale, "en");
    assert_eq!(c.site.timezone, "UTC");
    assert_eq!(c.server.listen, "127.0.0.1:8080".parse().unwrap());
    assert!(c.server.trusted_proxies.is_empty());
    assert_eq!(c.database.url.expose(), "sqlite://ferrite.db");
    assert_eq!(c.database.max_connections, 16);
    assert!(c.database.auto_migrate);
    assert!(!c.database.allow_contract);
    assert_eq!(c.database.schema, None);
    assert_eq!(
        c.media,
        MediaSection::Local {
            root: dir.path().join("media"),
            local_shared: false
        }
    );
    assert_eq!(c.cluster.mode, ClusterMode::Auto);
    assert_eq!(c.cluster.node_name, None);
    assert_eq!(c.cache.max_bytes, 256 << 20);
    assert_eq!(c.cache.logged_in, LoggedInCache::Bypass);
    assert_eq!(c.cache.poll_interval_ms, 250);
    assert_eq!(c.plugins.default_memory, 32 << 20);
    assert_eq!(c.plugins.default_fuel, 50_000_000);
    assert_eq!(c.plugins.render_deadline_ms, 200);
    assert_eq!(c.plugins.action_deadline_ms, 5000);
    assert_eq!(c.plugins.disable_after_failures, 20);
    assert_eq!(c.plugins.failure_window_secs, 600);
    assert_eq!(
        c.database_kind(),
        DbKind::Sqlite {
            path: dir.path().join("ferrite.db")
        }
    );
    assert_eq!(c.effective_cluster_mode(), ClusterMode::Solo);
}

#[test]
fn missing_file_reports_required_key() {
    let (_d, r) = run(None, &[]);
    let e = r.unwrap_err();
    assert_eq!(paths(&e), ["site.url"]);
    let (_d, r) = run(None, &[("FERRITE_SITE__URL", "http://localhost:8080")]);
    assert!(r.is_ok());
}

#[test]
fn env_overrides_nested() {
    let (_d, r) = run(
        Some("[site]\nurl = \"https://example.com\"\n[database]\nmax_connections = 8\n"),
        &[
            ("FERRITE_DATABASE__MAX_CONNECTIONS", "4"),
            ("FERRITE_SITE__TITLE", "2024"),
            ("FERRITE_SERVER__TRUSTED_PROXIES", "[\"10.0.0.0/8\"]"),
            ("FERRITE_SERVER__LISTEN", "0.0.0.0:9000"),
            ("FERRITE_CACHE__MAX_BYTES", "64MiB"),
            ("FERRITE_DATABASE__AUTO_MIGRATE", "false"),
            ("FERRITE_LOG", "debug"),
            ("FERRITE_TEST_PG_URL", "postgres://x"),
            ("HOME", "/root"),
        ],
    );
    let c = r.unwrap();
    assert_eq!(c.database.max_connections, 4);
    assert_eq!(c.site.title, "2024");
    assert_eq!(c.server.trusted_proxies, ["10.0.0.0/8".parse().unwrap()]);
    assert_eq!(c.server.listen.port(), 9000);
    assert_eq!(c.cache.max_bytes, 64 << 20);
    assert!(!c.database.auto_migrate);
}

#[test]
fn file_secret() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("ferrite.toml"),
        format!("{MIN}[database]\nurl_file = \"db_url\"\n"),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("db_url"),
        "postgres://u:hunter2@db/ferrite\r\n",
    )
    .unwrap();
    let c = load(dir.path(), env(&[])).unwrap();
    assert_eq!(c.database.url.expose(), "postgres://u:hunter2@db/ferrite");
    assert!(matches!(c.database_kind(), DbKind::Postgres { .. }));
    assert_eq!(c.effective_cluster_mode(), ClusterMode::Solo);
}

#[test]
fn secret_conflicts() {
    // Missing file.
    let (_d, r) = run(
        Some(&format!(
            "{MIN}[database]\nurl_file = \"/nonexistent/db_url\"\n"
        )),
        &[],
    );
    let e = r.unwrap_err();
    assert_eq!(paths(&e), ["database.url_file"]);

    // Env url + file url_file.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("ferrite.toml"),
        format!("{MIN}[database]\nurl_file = \"f\"\n"),
    )
    .unwrap();
    std::fs::write(dir.path().join("f"), "postgres://u:pw-in-file@h/d").unwrap();
    let e = load(
        dir.path(),
        env(&[("FERRITE_DATABASE__URL", "postgres://u:pw-in-env@h/d")]),
    )
    .unwrap_err();
    assert_eq!(e.problems.len(), 1);
    assert_eq!(e.problems[0].path, "env:FERRITE_DATABASE__URL");
    let text = format!("{e} {e:?}");
    assert!(!text.contains("pw-in-env") && !text.contains("pw-in-file"));

    // Both in the file.
    let (_d, r) = run(
        Some(&format!(
            "{MIN}[database]\nurl = \"sqlite://a.db\"\nurl_file = \"f\"\n"
        )),
        &[],
    );
    assert_eq!(paths(&r.unwrap_err()), ["database.url"]);

    // Empty secret file.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("ferrite.toml"),
        format!("{MIN}[database]\nurl_file = \"f\"\n"),
    )
    .unwrap();
    std::fs::write(dir.path().join("f"), "\n").unwrap();
    assert_eq!(
        paths(&load(dir.path(), env(&[])).unwrap_err()),
        ["database.url_file"]
    );
}

#[test]
fn s3_secret_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("ferrite.toml"),
        format!("{MIN}[media]\nbackend = \"s3\"\n[media.s3]\nbucket = \"b\"\nregion = \"r\"\naccess_key_id_file = \"k\"\nsecret_access_key_file = \"s\"\nprefix = \"p/\"\n"),
    )
    .unwrap();
    std::fs::write(dir.path().join("k"), "AKIA\n").unwrap();
    std::fs::write(dir.path().join("s"), "sekrit\n").unwrap();
    let c = load(dir.path(), env(&[])).unwrap();
    let MediaSection::S3 {
        access_key_id,
        secret_access_key,
        prefix,
        ..
    } = &c.media
    else {
        panic!("expected s3")
    };
    assert_eq!(access_key_id.as_deref(), Some("AKIA"));
    assert_eq!(
        secret_access_key.as_ref().map(Secret::expose),
        Some("sekrit")
    );
    assert_eq!(prefix, "p/");
    assert!(!format!("{c:?}").contains("sekrit"));
}

#[test]
fn collects_all_problems() {
    let (_d, r) = run(
        Some(
            "[server]\nlisten = \"nope\"\n[database]\nmax_connections = 0\n[cache]\nmax_bytes = \"12XB\"\npoll_interval_ms = 0\n",
        ),
        &[],
    );
    let e = r.unwrap_err();
    let p = paths(&e);
    assert!(p.contains(&"site.url"), "{p:?}");
    assert!(p.contains(&"server.listen"), "{p:?}");
    assert!(p.contains(&"database.max_connections"), "{p:?}");
    assert!(p.contains(&"cache.max_bytes"), "{p:?}");
    assert!(p.contains(&"cache.poll_interval_ms"), "{p:?}");
    assert_eq!(p.len(), 5, "{p:?}");
}

#[test]
fn cluster_validation() {
    let (_d, r) = run(Some(&format!("{MIN}[cluster]\nmode = \"cluster\"\n")), &[]);
    let p = paths(&r.unwrap_err()).join(",");
    assert!(
        p.contains("cluster.mode") && p.contains("media.backend"),
        "{p}"
    );

    // Postgres + S3 is fine.
    let ok = format!(
        "{MIN}[cluster]\nmode = \"cluster\"\n[database]\nurl = \"postgres://h/d\"\n[media]\nbackend = \"s3\"\n[media.s3]\nbucket = \"b\"\nregion = \"r\"\n"
    );
    assert!(run(Some(&ok), &[]).1.is_ok());

    // Postgres + shared local volume is fine.
    let shared = format!(
        "{MIN}[cluster]\nmode = \"cluster\"\n[database]\nurl = \"postgres://h/d\"\n[media]\nlocal_shared = true\n"
    );
    assert!(run(Some(&shared), &[]).1.is_ok());
}

#[test]
fn media_backend_keys() {
    let (_d, r) = run(
        Some(&format!(
            "{MIN}[media]\nbackend = \"local\"\n[media.s3]\nbucket = \"b\"\n"
        )),
        &[],
    );
    assert_eq!(paths(&r.unwrap_err()), ["media.s3"]);
    let (_d, r) = run(
        Some(&format!("{MIN}[media]\nbackend = \"s3\"\nroot = \"x\"\n")),
        &[],
    );
    let e = r.unwrap_err();
    assert_eq!(
        paths(&e),
        ["media.root", "media.s3.bucket", "media.s3.region"]
    );
}

#[test]
fn unknown_keys() {
    let (_d, r) = run(
        Some(&format!(
            "{MIN}bogus = 1\n[server]\nlistn = \"x\"\n[wat]\nx = 1\n"
        )),
        &[
            ("FERRITE_DATABASE__BOGUS", "1"),
            ("FERRITE_NOPE__X", "1"),
            ("FERRITE_SITE__TITL", "x"),
        ],
    );
    let e = r.unwrap_err();
    let mut p = paths(&e);
    p.sort();
    assert_eq!(
        p,
        [
            "env:FERRITE_DATABASE__BOGUS",
            "env:FERRITE_NOPE__X",
            "env:FERRITE_SITE__TITL",
            "server.listn",
            "site.bogus",
            "wat"
        ]
    );
}

#[test]
fn debug_redacts_secrets() {
    let (_d, r) = run(
        Some(&format!(
            "{MIN}[database]\nurl = \"postgres://u:topsecretpw@h/d\"\n"
        )),
        &[],
    );
    let c = r.unwrap();
    assert!(!format!("{c:?}").contains("topsecretpw"));
    assert!(!format!("{:?}", c.database_kind()).contains("topsecretpw"));
}

#[test]
fn problem_messages_do_not_leak_database_url() {
    let (_d, r) = run(
        Some(&format!(
            "{MIN}[database]\nurl = \"mysql://u:leakme@h/d\"\n"
        )),
        &[],
    );
    let e = r.unwrap_err();
    assert_eq!(paths(&e), ["database.url"]);
    assert!(!format!("{e} {e:?}").contains("leakme"));

    let (_d, r) = run(
        None,
        &[
            ("FERRITE_SITE__URL", "https://e.com"),
            ("FERRITE_DATABASE__URL", "mysql://u:leakme@h/d"),
        ],
    );
    let e = r.unwrap_err();
    assert_eq!(paths(&e), ["env:FERRITE_DATABASE__URL"]);
    assert!(!format!("{e} {e:?}").contains("leakme"));

    // Invalid TOML must not echo source.
    let (_d, r) = run(Some("[database]\nurl = \"leakme\nx"), &[]);
    let e = r.unwrap_err();
    assert!(!format!("{e}").contains("leakme"));
}

#[test]
fn sizes_and_durations() {
    let (_d, r) = run(
        Some(&format!(
            "{MIN}[cache]\nmax_bytes = 1024\npoll_interval_ms = 50\n[plugins]\ndefault_memory = \"32MiB\"\nfailure_window = \"2h\"\n"
        )),
        &[],
    );
    let c = r.unwrap();
    assert_eq!(c.cache.max_bytes, 1024);
    assert_eq!(c.cache.poll_interval_ms, 50);
    assert_eq!(c.plugins.default_memory, 32 << 20);
    assert_eq!(c.plugins.failure_window_secs, 7200);
}

#[test]
fn render_default_toml_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let rendered = render_default_toml(
        "https://example.com",
        "My \"quoted\" site",
        Some("postgres://u:p@h/d"),
        Some("my_schema"),
        &InitMedia::Local,
    );
    std::fs::write(dir.path().join("ferrite.toml"), &rendered).unwrap();
    let c = load(dir.path(), env(&[])).unwrap();
    assert_eq!(c.site.title, "My \"quoted\" site");
    assert_eq!(c.database.url.expose(), "postgres://u:p@h/d");
    assert_eq!(c.database.schema.as_deref(), Some("my_schema"));

    let minimal = render_default_toml("https://example.com", "", None, None, &InitMedia::Local);
    std::fs::write(dir.path().join("ferrite.toml"), minimal).unwrap();
    let c2 = load(dir.path(), env(&[])).unwrap();
    let (_d, c3) = run(Some(MIN), &[]);
    let c3 = c3.unwrap();
    assert_eq!(c2.site, c3.site);
    assert_eq!(c2.database, c3.database);
    assert_eq!(c2.cache, c3.cache);
    assert_eq!(c2.plugins, c3.plugins);
    assert_eq!(c2.server, c3.server);
    assert_eq!(c2.cluster, c3.cluster);
    assert!(matches!(c2.media, MediaSection::Local { .. }));
}

fn auto_mode(db: &str, media: &str) -> ClusterMode {
    let t = format!(
        "{MIN}[database]\nurl = \"{db}\"\n[media]\n{media}\n[media.s3]\nbucket = \"b\"\nregion = \"r\"\n"
    );
    // Drop the [media.s3] table for local variants.
    let t = if media.contains("local") {
        t.split("[media.s3]").next().unwrap().to_owned()
    } else {
        t
    };
    run(Some(&t), &[]).1.unwrap().effective_cluster_mode()
}

#[test]
fn auto_cluster_mode_rule() {
    assert_eq!(
        auto_mode("postgres://h/d", "backend = \"s3\""),
        ClusterMode::Cluster
    );
    assert_eq!(
        auto_mode("postgres://h/d", "backend = \"local\"\nlocal_shared = true"),
        ClusterMode::Cluster
    );
    assert_eq!(
        auto_mode("postgres://h/d", "backend = \"local\""),
        ClusterMode::Solo
    );
    assert_eq!(
        auto_mode("sqlite://a.db", "backend = \"s3\""),
        ClusterMode::Solo
    );
}

#[test]
fn env_string_values_are_raw() {
    let (_d, r) = run(
        Some(MIN),
        &[
            ("FERRITE_DATABASE__URL", "postgres://u:p@h/d"),
            ("FERRITE_SITE__TITLE", "\"abc\""),
        ],
    );
    let c = r.unwrap();
    assert_eq!(c.database.url.expose(), "postgres://u:p@h/d");
    assert_eq!(c.site.title, "\"abc\"");

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ferrite.toml"), MIN).unwrap();
    let c = load(
        dir.path(),
        env(&[
            ("FERRITE_DATABASE__URL", "'abc'\nsecond"),
            ("FERRITE_MEDIA__BACKEND", "local"),
        ]),
    );
    // Unsupported scheme is a problem, but the multi-line value must not truncate parsing.
    assert_eq!(paths(&c.unwrap_err()), ["env:FERRITE_DATABASE__URL"]);

    let (_d, r) = run(
        Some(MIN),
        &[("FERRITE_DATABASE__URL", "postgres://u:'q\"x'@h/d")],
    );
    assert_eq!(r.unwrap().database.url.expose(), "postgres://u:'q\"x'@h/d");
}

#[test]
fn cluster_problems_collected_with_others() {
    let (_d, r) = run(Some("[cluster]\nmode = \"cluster\"\n"), &[]);
    let p = paths(&r.unwrap_err()).join(",");
    assert!(
        p.contains("site.url") && p.contains("cluster.mode") && p.contains("media.backend"),
        "{p}"
    );
}

#[test]
fn extra_validation() {
    let (_d, r) = run(
        Some(&format!(
            "{MIN}[database]\nurl = \"sqlite://a.db?mode=ro\"\nschema = \"s\"\n[media]\nbackend = \"s3\"\n[media.s3]\nbucket = \"b\"\nregion = \"r\"\nendpoint = \"not a url\"\n"
        )),
        &[],
    );
    let e = r.unwrap_err();
    assert_eq!(
        paths(&e),
        ["database.url", "database.schema", "media.s3.endpoint"]
    );
}

#[test]
fn render_default_toml_s3_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    for (url, bucket, prefix) in [
        ("s3://my-bucket/a/b", "my-bucket", "a/b/"),
        ("s3://my-bucket/a/b/", "my-bucket", "a/b/"),
        ("s3://my-bucket", "my-bucket", ""),
        ("s3://my-bucket/", "my-bucket", ""),
    ] {
        let media = InitMedia::parse_s3_url(url).unwrap();
        let rendered = render_default_toml("https://example.com", "t", None, None, &media);
        std::fs::write(dir.path().join("ferrite.toml"), rendered).unwrap();
        let c = load(dir.path(), env(&[])).unwrap();
        let MediaSection::S3 {
            bucket: b,
            prefix: p,
            region,
            ..
        } = c.media
        else {
            panic!("expected s3")
        };
        assert_eq!(
            (b.as_str(), p.as_str(), region.as_str()),
            (bucket, prefix, "us-east-1")
        );
    }
    assert!(InitMedia::parse_s3_url("s3:///x").is_err());
    assert!(InitMedia::parse_s3_url("http://x").is_err());
}
