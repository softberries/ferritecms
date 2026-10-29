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
    assert_eq!(c.cache.search_ttl_secs, 60);
    assert_eq!(c.plugins.volatile_ttl_secs, 60);
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
    let key = [("FERRITE_AUTH__SECRET_KEY", KEY)];
    assert!(run(Some(&ok), &key).1.is_ok());

    // Postgres + shared local volume is fine.
    let shared = format!(
        "{MIN}[cluster]\nmode = \"cluster\"\n[database]\nurl = \"postgres://h/d\"\n[media]\nlocal_shared = true\n"
    );
    assert!(run(Some(&shared), &key).1.is_ok());
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
    assert_eq!(c2.auth, c3.auth);
    assert_eq!(c2.uploads, c3.uploads);
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
    // The key is required when the result is cluster mode.
    run(Some(&t), &[("FERRITE_AUTH__SECRET_KEY", KEY)])
        .1
        .unwrap()
        .effective_cluster_mode()
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

#[test]
fn schema_longer_than_48_is_rejected() {
    let toml = |schema: &str| {
        format!("{MIN}[database]\nurl = \"postgres://u@h/d\"\nschema = \"{schema}\"\n")
    };
    let (_d, r) = run(Some(&toml(&"s".repeat(48))), &[]);
    assert_eq!(
        r.unwrap().database.schema.as_deref(),
        Some("s".repeat(48).as_str())
    );

    let (_d, r) = run(Some(&toml(&"s".repeat(49))), &[]);
    let e = r.unwrap_err();
    assert_eq!(paths(&e), ["database.schema"]);
    assert!(
        e.problems[0].message.contains("at most 48 characters"),
        "{}",
        e.problems[0].message
    );
}

#[test]
fn workers_default_and_bounds() {
    let (_d, r) = run(Some(MIN), &[]);
    let c = r.unwrap();
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    assert_eq!(c.cluster.workers, cpus.max(2));
    assert!(c.cluster.workers >= 2);

    for bad in ["0", "257", "-1", "\"4\""] {
        let (_d, r) = run(Some(&format!("{MIN}[cluster]\nworkers = {bad}\n")), &[]);
        let e = r.unwrap_err();
        assert_eq!(paths(&e), ["cluster.workers"], "{bad}");
        if bad == "0" || bad == "257" {
            assert_eq!(e.problems[0].message, "must be between 1 and 256", "{bad}");
        }
    }
    for (ok, want) in [("1", 1), ("256", 256)] {
        let (_d, r) = run(Some(&format!("{MIN}[cluster]\nworkers = {ok}\n")), &[]);
        assert_eq!(r.unwrap().cluster.workers, want);
    }
    let (_d, r) = run(Some(MIN), &[("FERRITE_CLUSTER__WORKERS", "3")]);
    assert_eq!(r.unwrap().cluster.workers, 3);
}

#[test]
fn node_name_trimmed_and_bounded() {
    let (_d, r) = run(
        Some(&format!("{MIN}[cluster]\nnode_name = \"  web-1 \"\n")),
        &[],
    );
    assert_eq!(r.unwrap().cluster.node_name.as_deref(), Some("web-1"));
    for bad in ["", "   ", &"x".repeat(65)] {
        let (_d, r) = run(
            Some(&format!("{MIN}[cluster]\nnode_name = \"{bad}\"\n")),
            &[],
        );
        assert_eq!(paths(&r.unwrap_err()), ["cluster.node_name"], "{bad:?}");
    }
    let (_d, r) = run(
        Some(&format!(
            "{MIN}[cluster]\nnode_name = \"{}\"\n",
            "x".repeat(64)
        )),
        &[],
    );
    assert_eq!(r.unwrap().cluster.node_name.map(|n| n.len()), Some(64));
}

const KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

#[test]
fn auth_secret_key_rules() {
    // Inline, env and file forms.
    let (_d, r) = run(Some(&format!("{MIN}[auth]\nsecret_key = \"{KEY}\"\n")), &[]);
    let c = r.unwrap();
    assert_eq!(c.auth.secret_key.as_ref().map(Secret::expose), Some(KEY));
    assert!(!format!("{c:?}").contains(KEY));
    let upper = KEY.to_uppercase();
    let (_d, r) = run(Some(MIN), &[("FERRITE_AUTH__SECRET_KEY", &upper)]);
    assert!(r.is_ok());
    // All-digit and float-looking values stay raw strings from the environment.
    let digits = "1".repeat(64);
    let (_d, r) = run(Some(MIN), &[("FERRITE_AUTH__SECRET_KEY", &digits)]);
    assert_eq!(
        r.unwrap().auth.secret_key.as_ref().map(Secret::expose),
        Some(digits.as_str())
    );
    let float = format!("1e{}", "0".repeat(62));
    let (_d, r) = run(Some(MIN), &[("FERRITE_AUTH__SECRET_KEY", &float)]);
    assert!(r.is_ok());

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("ferrite.toml"),
        format!("{MIN}[auth]\nsecret_key_file = \"site.key\"\n"),
    )
    .unwrap();
    std::fs::write(dir.path().join("site.key"), format!("{KEY}\n")).unwrap();
    let c = load(dir.path(), env(&[])).unwrap();
    assert_eq!(c.auth.secret_key.as_ref().map(Secret::expose), Some(KEY));
    std::fs::write(dir.path().join("site.key"), "nothex\n").unwrap();
    let e = load(dir.path(), env(&[])).unwrap_err();
    assert_eq!(paths(&e), ["auth.secret_key_file"]);

    // Wrong length or not hex; the message never echoes the value.
    let g = format!("{}g", &KEY[..63]);
    for bad in [&KEY[..63], &format!("{KEY}0"), g.as_str(), ""] {
        let (_d, r) = run(Some(&format!("{MIN}[auth]\nsecret_key = \"{bad}\"\n")), &[]);
        let e = r.unwrap_err();
        assert_eq!(paths(&e), ["auth.secret_key"], "{bad:?}");
        assert_eq!(
            e.problems[0].message, "must be 64 hex characters (32 bytes)",
            "{bad:?}"
        );
        if !bad.is_empty() {
            assert!(!format!("{e}").contains(bad), "{e}");
        }
    }
    let (_d, r) = run(Some(MIN), &[("FERRITE_AUTH__SECRET_KEY", "abc")]);
    assert_eq!(paths(&r.unwrap_err()), ["env:FERRITE_AUTH__SECRET_KEY"]);
    let (_d, r) = run(Some(&format!("{MIN}[auth]\nbogus = 1\n")), &[]);
    assert_eq!(paths(&r.unwrap_err()), ["auth.bogus"]);

    // Solo without a key is fine (the binary uses <data>/secret.key).
    let (_d, r) = run(Some(MIN), &[]);
    assert_eq!(r.unwrap().auth.secret_key, None);

    // Cluster (explicit, or auto resolving to cluster) requires it.
    let cluster = format!(
        "{MIN}[cluster]\nmode = \"cluster\"\n[database]\nurl = \"postgres://h/d\"\n[media]\nlocal_shared = true\n"
    );
    let auto = format!(
        "{MIN}[database]\nurl = \"postgres://h/d\"\n[media]\nbackend = \"s3\"\n[media.s3]\nbucket = \"b\"\nregion = \"r\"\n"
    );
    for toml in [&cluster, &auto] {
        let (_d, r) = run(Some(toml), &[]);
        let e = r.unwrap_err();
        assert_eq!(paths(&e), ["auth.secret_key"]);
        assert_eq!(
            e.problems[0].message,
            "required in cluster mode (every node must use the same key)"
        );
        let (_d, r) = run(Some(toml), &[("FERRITE_AUTH__SECRET_KEY", KEY)]);
        assert_eq!(r.unwrap().effective_cluster_mode(), ClusterMode::Cluster);
    }
}

#[test]
fn mail_section_rules() {
    // Absent: no mail.
    let (_d, r) = run(Some(MIN), &[]);
    assert_eq!(r.unwrap().mail, None);

    let ok = format!(
        "{MIN}[mail]\nsmtp_url = \"smtps://user:topsecretpw@mail.example.com:465\"\nfrom = \"Site <noreply@example.com>\"\n"
    );
    let (_d, r) = run(Some(&ok), &[]);
    let mail = r.unwrap().mail.unwrap();
    assert_eq!(
        mail.smtp_url.expose(),
        "smtps://user:topsecretpw@mail.example.com:465"
    );
    assert_eq!(mail.from, "Site <noreply@example.com>");
    assert!(!format!("{mail:?}").contains("topsecretpw"));
    for url in [
        "smtp://127.0.0.1:2525",
        "smtp://mail.example.com?tls=required",
    ] {
        let (_d, r) = run(
            Some(&format!(
                "{MIN}[mail]\nsmtp_url = \"{url}\"\nfrom = \"a@example.com\"\n"
            )),
            &[],
        );
        assert!(r.is_ok(), "{url}: {r:?}");
    }

    // Bad schemes and shapes; the URL (and its password) is never echoed.
    for url in [
        "http://user:topsecretpw@mail.example.com",
        "smtp:topsecretpw",
        "smtp://user:topsecretpw@/",
        "not a url topsecretpw",
        "smtp://user:topsecretpw@mail.example.com?tls=sometimes",
        // R16: no opportunistic mode (it falls back to plaintext).
        "smtp://user:topsecretpw@mail.example.com?tls=opportunistic",
        "smtp://mail.example.com?tls=opportunistic topsecretpw",
        "smtps://user:topsecretpw@mail.example.com?tls=required",
    ] {
        let (_d, r) = run(
            Some(&format!(
                "{MIN}[mail]\nsmtp_url = \"{url}\"\nfrom = \"a@example.com\"\n"
            )),
            &[],
        );
        let e = r.unwrap_err();
        assert_eq!(paths(&e), ["mail.smtp_url"], "{url}");
        assert!(!e.to_string().contains("topsecretpw"), "{e}");
        assert!(!e.to_string().contains("mail.example.com"), "{e}");
    }

    // `from` is required with a URL, must be a mailbox, and needs the URL.
    let (_d, r) = run(
        Some(&format!("{MIN}[mail]\nsmtp_url = \"smtp://h:25\"\n")),
        &[],
    );
    assert_eq!(paths(&r.unwrap_err()), ["mail.from"]);
    for from in [
        "",
        "nobody",
        "a@b@c",
        "Name <a@example.com",
        "a@example.com\r\nBcc: x@evil.test",
        "Na\nme <a@example.com>",
        "<>",
    ] {
        let toml = format!(
            "{MIN}[mail]\nsmtp_url = \"smtp://h:25\"\nfrom = {}\n",
            toml::Value::String(from.into())
        );
        let (_d, r) = run(Some(&toml), &[]);
        assert_eq!(paths(&r.unwrap_err()), ["mail.from"], "{from:?}");
    }
    let (_d, r) = run(
        Some(&format!("{MIN}[mail]\nfrom = \"a@example.com\"\n")),
        &[],
    );
    assert_eq!(paths(&r.unwrap_err()), ["mail.from"]);

    // `_file` works (one trailing newline stripped); both at once is a problem.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("smtp.url"),
        "smtp://u:topsecretpw@127.0.0.1:2525\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("ferrite.toml"),
        format!("{MIN}[mail]\nsmtp_url_file = \"smtp.url\"\nfrom = \"a@example.com\"\n"),
    )
    .unwrap();
    let c = load(dir.path(), env(&[])).unwrap();
    assert_eq!(
        c.mail.unwrap().smtp_url.expose(),
        "smtp://u:topsecretpw@127.0.0.1:2525"
    );
    std::fs::write(dir.path().join("smtp.url"), "ftp://u:topsecretpw@h\n").unwrap();
    let e = load(dir.path(), env(&[])).unwrap_err();
    assert_eq!(paths(&e), ["mail.smtp_url_file"]);
    assert!(!e.to_string().contains("topsecretpw"), "{e}");
    std::fs::write(
        dir.path().join("ferrite.toml"),
        format!(
            "{MIN}[mail]\nsmtp_url = \"smtp://h\"\nsmtp_url_file = \"smtp.url\"\nfrom = \"a@example.com\"\n"
        ),
    )
    .unwrap();
    assert_eq!(
        paths(&load(dir.path(), env(&[])).unwrap_err()),
        ["mail.smtp_url"]
    );

    // From the environment, problems name the variable.
    let (_d, r) = run(
        Some(MIN),
        &[
            ("FERRITE_MAIL__SMTP_URL", "pop3://u:topsecretpw@h"),
            ("FERRITE_MAIL__FROM", "a@example.com"),
        ],
    );
    let e = r.unwrap_err();
    assert_eq!(paths(&e), ["env:FERRITE_MAIL__SMTP_URL"]);
    assert!(!e.to_string().contains("topsecretpw"), "{e}");
    // Unknown keys are refused.
    let (_d, r) = run(
        Some(&format!(
            "{MIN}[mail]\nsmtp_url = \"smtp://h\"\nfrom = \"a@example.com\"\nport = 25\n"
        )),
        &[],
    );
    assert_eq!(paths(&r.unwrap_err()), ["mail.port"]);
}

/// R16: credentials in the SMTP URL only over TLS (`smtps`, `tls=required`) or on plain
/// `smtp` to a loopback relay; `config check` names the reason, never the URL.
#[test]
fn mail_credentials_need_tls() {
    let check = |url: &str| {
        let (_d, r) = run(
            Some(&format!(
                "{MIN}[mail]\nsmtp_url = \"{url}\"\nfrom = \"a@example.com\"\n"
            )),
            &[],
        );
        r
    };
    for url in [
        "smtps://user:topsecretpw@mail.example.com",
        "smtp://user:topsecretpw@mail.example.com:587?tls=required",
        "smtp://user:topsecretpw@localhost:25",
        "smtp://user:topsecretpw@LOCALHOST",
        "smtp://user:topsecretpw@127.0.0.1:2525",
        "smtp://user:topsecretpw@127.1.2.3",
        "smtp://user:topsecretpw@[::1]:25",
        "smtp://mail.example.com:25",
    ] {
        assert!(check(url).is_ok(), "{url}");
    }
    for url in [
        "smtp://user:topsecretpw@mail.example.com:25",
        "smtp://user@mail.example.com",
        "smtp://:topsecretpw@mail.example.com",
        "smtp://user:topsecretpw@10.0.0.1",
        "smtp://user:topsecretpw@localhost.example.com",
        "smtp://user:topsecretpw@[::2]",
    ] {
        let e = check(url).unwrap_err();
        assert_eq!(paths(&e), ["mail.smtp_url"], "{url}");
        let text = e.to_string();
        assert!(text.contains("credentials need TLS"), "{url}: {text}");
        assert!(
            !text.contains("topsecretpw") && !text.contains("example.com:"),
            "{text}"
        );
    }
    let e = check("smtp://user:topsecretpw@mail.example.com?tls=opportunistic").unwrap_err();
    assert!(
        e.to_string().contains("`tls` may only be `required`"),
        "{e}"
    );
    assert_eq!(check_smtp_url("smtp://u:p@h.example?tls=required"), Ok(()));
    assert!(check_smtp_url("smtp://u:p@h.example").is_err());
}

/// P22: `plugins.max_concurrent_calls` (default `max(8, 4 × available_parallelism)`) and
/// `plugins.render_budget_ms` (default 1 000), both > 0.
#[test]
fn plugins_limits_rules() {
    let (_d, r) = run(Some(MIN), &[]);
    let c = r.unwrap();
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    assert_eq!(c.plugins.max_concurrent_calls, (4 * cpus).max(8));
    assert!(c.plugins.max_concurrent_calls >= 8);
    assert_eq!(c.plugins.render_budget_ms, 1000);

    for key in ["max_concurrent_calls", "render_budget_ms"] {
        for bad in ["0", "-1", "\"4\"", "1.5"] {
            let (_d, r) = run(Some(&format!("{MIN}[plugins]\n{key} = {bad}\n")), &[]);
            let e = r.unwrap_err();
            assert_eq!(
                paths(&e),
                [format!("plugins.{key}").as_str()],
                "{key} = {bad}"
            );
        }
    }
    let (_d, r) = run(
        Some(&format!(
            "{MIN}[plugins]\nmax_concurrent_calls = 1\nrender_budget_ms = 300\n"
        )),
        &[],
    );
    let c = r.unwrap();
    assert_eq!(c.plugins.max_concurrent_calls, 1);
    assert_eq!(c.plugins.render_budget_ms, 300);
    let (_d, r) = run(
        Some(MIN),
        &[
            ("FERRITE_PLUGINS__MAX_CONCURRENT_CALLS", "3"),
            ("FERRITE_PLUGINS__RENDER_BUDGET_MS", "250"),
        ],
    );
    let c = r.unwrap();
    assert_eq!(c.plugins.max_concurrent_calls, 3);
    assert_eq!(c.plugins.render_budget_ms, 250);

    // Review m2/m3: at most one minute, and never below the render deadline.
    let problem = |toml: &str| {
        let (_d, r) = run(Some(&format!("{MIN}[plugins]\n{toml}")), &[]);
        let e = r.unwrap_err();
        assert_eq!(paths(&e), ["plugins.render_budget_ms"], "{toml}");
        e.problems[0].message.clone()
    };
    assert_eq!(
        problem("render_budget_ms = 60001\n"),
        "must be at most 60000"
    );
    assert_eq!(
        problem("render_budget_ms = 199\n"),
        "must be at least plugins.render_deadline_ms (200)"
    );
    assert_eq!(
        problem("render_deadline_ms = 500\nrender_budget_ms = 400\n"),
        "must be at least plugins.render_deadline_ms (500)"
    );
    for ok in [
        "render_budget_ms = 60000\n",
        "render_budget_ms = 200\n",
        "render_deadline_ms = 100\nrender_budget_ms = 150\n",
    ] {
        let (_d, r) = run(Some(&format!("{MIN}[plugins]\n{ok}")), &[]);
        assert!(r.is_ok(), "{ok}: {r:?}");
    }
}

#[test]
fn media_upload_rules() {
    // Defaults: 256 MiB, one image worker.
    let (_d, r) = run(Some(MIN), &[]);
    let c = r.unwrap();
    assert_eq!(c.uploads.max_upload, 256 << 20);
    assert_eq!(c.uploads.image_workers, 1);
    assert_eq!(c.uploads.idle_timeout_secs, 30);
    assert!(!c.uploads.strip_metadata);
    assert_eq!(c.uploads.gc_grace_days, 7);
    assert_eq!(DEFAULT_MAX_UPLOAD, 256 << 20);
    assert_eq!(MAX_UPLOAD_CEILING, 1 << 30);

    // Both backends' [media] sections accept the keys.
    for backend in [
        "backend = \"local\"\n",
        "backend = \"s3\"\n[media.s3]\nbucket = \"b\"\nregion = \"r\"\n",
    ] {
        let (_d, r) = run(
            Some(&format!(
                "{MIN}[media]\nmax_upload = \"1GiB\"\nimage_workers = 16\nupload_idle_timeout = \"2m\"\n{backend}"
            )),
            &[],
        );
        let c = r.unwrap_or_else(|e| panic!("{backend}: {e:?}"));
        assert_eq!(c.uploads.max_upload, 1 << 30, "{backend}");
        assert_eq!(c.uploads.image_workers, 16, "{backend}");
        assert_eq!(c.uploads.idle_timeout_secs, 120, "{backend}");
    }
    let (_d, r) = run(Some(&format!("{MIN}[media]\nmax_upload = 1\n")), &[]);
    assert_eq!(r.unwrap().uploads.max_upload, 1);
    let (_d, r) = run(Some(MIN), &[("FERRITE_MEDIA__MAX_UPLOAD", "8MB")]);
    assert_eq!(r.unwrap().uploads.max_upload, 8 << 20);
    let (_d, r) = run(Some(&format!("{MIN}[media]\nstrip_metadata = true\n")), &[]);
    assert!(r.unwrap().uploads.strip_metadata);
    let (_d, r) = run(Some(MIN), &[("FERRITE_MEDIA__STRIP_METADATA", "true")]);
    assert!(r.unwrap().uploads.strip_metadata);
    for (toml, days) in [("gc_grace_days = 1\n", 1), ("gc_grace_days = 365\n", 365)] {
        let (_d, r) = run(Some(&format!("{MIN}[media]\n{toml}")), &[]);
        assert_eq!(r.unwrap().uploads.gc_grace_days, days, "{toml}");
    }
    let (_d, r) = run(Some(MIN), &[("FERRITE_MEDIA__GC_GRACE_DAYS", "30")]);
    assert_eq!(r.unwrap().uploads.gc_grace_days, 30);

    let problem = |toml: &str| {
        let (_d, r) = run(Some(&format!("{MIN}[media]\n{toml}")), &[]);
        let e = r.unwrap_err();
        assert_eq!(e.problems.len(), 1, "{toml}: {e:?}");
        (e.problems[0].path.clone(), e.problems[0].message.clone())
    };
    let (path, _) = problem("max_upload = 0\n");
    assert_eq!(path, "media.max_upload");
    assert_eq!(
        problem(&format!("max_upload = {}\n", (1u64 << 30) + 1)),
        ("media.max_upload".into(), "must be at most 1GiB".into())
    );
    assert_eq!(
        problem("max_upload = \"2GB\"\n"),
        ("media.max_upload".into(), "must be at most 1GiB".into())
    );
    let (path, _) = problem("upload_idle_timeout = 0\n");
    assert_eq!(path, "media.upload_idle_timeout");
    for bad in ["gc_grace_days = 0\n", "gc_grace_days = 366\n"] {
        assert_eq!(
            problem(bad),
            (
                "media.gc_grace_days".into(),
                "must be between 1 and 365".into()
            ),
            "{bad}"
        );
    }
    for bad in ["image_workers = 0\n", "image_workers = 17\n"] {
        assert_eq!(
            problem(bad),
            (
                "media.image_workers".into(),
                "must be between 1 and 16".into()
            ),
            "{bad}"
        );
    }
}

/// `cache.search_ttl` (default 60 s, 0..=1 h, `0` = search pages are not cached) and
/// `plugins.volatile_ttl` (default 60 s, 1 s..=1 h); both durations.
#[test]
fn ttl_rules() {
    let (_d, r) = run(Some(MIN), &[]);
    let c = r.unwrap();
    assert_eq!(
        (c.cache.search_ttl_secs, c.plugins.volatile_ttl_secs),
        (60, 60)
    );

    let ok = |toml: &str| {
        let (_d, r) = run(Some(&format!("{MIN}{toml}")), &[]);
        r.unwrap_or_else(|e| panic!("{toml}: {e}"))
    };
    assert_eq!(ok("[cache]\nsearch_ttl = 0\n").cache.search_ttl_secs, 0);
    assert_eq!(
        ok("[cache]\nsearch_ttl = \"0s\"\n").cache.search_ttl_secs,
        0
    );
    assert_eq!(
        ok("[cache]\nsearch_ttl = \"5m\"\n").cache.search_ttl_secs,
        300
    );
    assert_eq!(
        ok("[cache]\nsearch_ttl = \"1h\"\n").cache.search_ttl_secs,
        3600
    );
    assert_eq!(
        ok("[plugins]\nvolatile_ttl = 1\n")
            .plugins
            .volatile_ttl_secs,
        1
    );
    assert_eq!(
        ok("[plugins]\nvolatile_ttl = \"1h\"\n")
            .plugins
            .volatile_ttl_secs,
        3600
    );
    assert_eq!(
        ok("[plugins]\nvolatile_ttl = \"90s\"\n")
            .plugins
            .volatile_ttl_secs,
        90
    );

    let (_d, r) = run(
        Some(MIN),
        &[
            ("FERRITE_CACHE__SEARCH_TTL", "0"),
            ("FERRITE_PLUGINS__VOLATILE_TTL", "\"2m\""),
        ],
    );
    let c = r.unwrap();
    assert_eq!(
        (c.cache.search_ttl_secs, c.plugins.volatile_ttl_secs),
        (0, 120)
    );

    let problem = |toml: &str| {
        let (_d, r) = run(Some(&format!("{MIN}{toml}")), &[]);
        let e = r.unwrap_err();
        assert_eq!(e.problems.len(), 1, "{toml}: {e}");
        (e.problems[0].path.clone(), e.problems[0].message.clone())
    };
    for bad in ["-1", "\"soon\"", "1.5", "\"-5s\""] {
        let (path, _) = problem(&format!("[cache]\nsearch_ttl = {bad}\n"));
        assert_eq!(path, "cache.search_ttl", "{bad}");
    }
    for bad in ["0", "\"0s\"", "-1", "\"x\""] {
        let (path, _) = problem(&format!("[plugins]\nvolatile_ttl = {bad}\n"));
        assert_eq!(path, "plugins.volatile_ttl", "{bad}");
    }
    // Review round 1: search_ttl is capped at 1 h like volatile_ttl.
    assert_eq!(
        problem("[cache]\nsearch_ttl = 3601\n"),
        (
            "cache.search_ttl".into(),
            "must be at most 3600 seconds (1h)".into()
        )
    );
    assert_eq!(
        problem("[cache]\nsearch_ttl = \"2h\"\n").0,
        "cache.search_ttl"
    );
    assert_eq!(
        problem("[plugins]\nvolatile_ttl = 3601\n"),
        (
            "plugins.volatile_ttl".into(),
            "must be at most 3600 seconds (1h)".into()
        )
    );
    assert_eq!(
        problem("[plugins]\nvolatile_ttl = \"2h\"\n").0,
        "plugins.volatile_ttl"
    );
}

fn s3_toml(extra: &str) -> String {
    format!("{MIN}[media]\nbackend = \"s3\"\n[media.s3]\nbucket = \"b\"\nregion = \"r\"\n{extra}")
}

fn s3_copy_mode(r: Result<Config, ConfigError>) -> Option<String> {
    let MediaSection::S3 {
        copy_if_not_exists, ..
    } = r.unwrap().media
    else {
        panic!("expected s3")
    };
    copy_if_not_exists
}

#[test]
fn parse_s3_copy_if_not_exists_values() {
    for ok in [
        "header:x-amz-copy-source-if-none-match:*",
        "header-with-status:cf-if-none-match:*:412",
        "multipart",
    ] {
        let (_d, r) = run(
            Some(&s3_toml(&format!("copy_if_not_exists = \"{ok}\"\n"))),
            &[],
        );
        assert_eq!(s3_copy_mode(r).as_deref(), Some(ok));
    }
    let (_d, r) = run(Some(&s3_toml("")), &[]);
    assert_eq!(s3_copy_mode(r), None);
    for bad in [
        "nonsense",
        "header:only-name",
        "header-with-status:a:b:zzz",
        "",
    ] {
        let (_d, r) = run(
            Some(&s3_toml(&format!("copy_if_not_exists = \"{bad}\"\n"))),
            &[],
        );
        let e = r.unwrap_err();
        assert_eq!(paths(&e), ["media.s3.copy_if_not_exists"], "{bad:?}");
    }
    let (_d, r) = run(
        Some(&s3_toml("")),
        &[("FERRITE_MEDIA__S3__COPY_IF_NOT_EXISTS", "bogus")],
    );
    assert_eq!(
        paths(&r.unwrap_err()),
        ["env:FERRITE_MEDIA__S3__COPY_IF_NOT_EXISTS"]
    );
}

#[test]
fn conditional_put_disabled_parses() {
    for (value, want) in [
        (None, None),
        (Some("etag"), Some(ferrite_media::ConditionalPut::Etag)),
        (
            Some("disabled"),
            Some(ferrite_media::ConditionalPut::Disabled),
        ),
    ] {
        let extra = value.map_or(String::new(), |v| format!("conditional_put = \"{v}\"\n"));
        let (_d, r) = run(Some(&s3_toml(&extra)), &[]);
        let MediaSection::S3 {
            conditional_put, ..
        } = r.unwrap().media
        else {
            panic!("expected s3")
        };
        assert_eq!(conditional_put, want, "{value:?}");
    }
    let (_d, r) = run(Some(&s3_toml("conditional_put = \"always\"\n")), &[]);
    assert_eq!(paths(&r.unwrap_err()), ["media.s3.conditional_put"]);
}
