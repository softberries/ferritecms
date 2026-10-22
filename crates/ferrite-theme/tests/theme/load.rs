use ferrite_theme::{MAX_FILES, Theme, ThemeError, ThemeSource};

use crate::common::{MANIFEST, ctx, load, render, theme, write, write_theme};

#[test]
fn embedded_fixture_loads_and_renders() {
    let t = Theme::load(&ThemeSource::Embedded("test-minimal")).unwrap();
    assert_eq!(t.name(), "test-minimal");
    assert!(Theme::embedded_names().contains(&"test-minimal".to_owned()));
    assert_eq!(t.hash().len(), 16);
    assert!(t.hash().bytes().all(|b| b.is_ascii_hexdigit()));
    assert!(t.has_template("index.html"));
    assert!(t.has_template("partials/header.html"));
    let (css, mime) = t.asset("css/site.css").unwrap();
    assert!(std::str::from_utf8(css).unwrap().contains("color"));
    assert_eq!(mime, "text/css; charset=utf-8");
    assert_eq!(
        t.manifest().settings["accent_color"].default,
        Some(serde_json::json!("#c2410c"))
    );
    let out = render(&t, "index.html", ctx()).unwrap();
    assert!(out.html.contains("My &lt;Site&gt;"), "{}", out.html);
    assert!(
        out.html.contains(&format!(
            "&#x2f;_theme&#x2f;{}&#x2f;css&#x2f;site.css",
            t.hash()
        )),
        "{}",
        out.html
    );
}

#[test]
fn unknown_embedded_theme_is_not_found() {
    for name in ["nope", "", "../themes", "test-minimal/templates"] {
        assert!(
            matches!(
                Theme::load(&ThemeSource::Embedded(name)),
                Err(ThemeError::NotFound(_))
            ),
            "{name}"
        );
    }
}

#[test]
fn assets_only_from_assets_folder() {
    let t = theme(&[
        ("assets/app.js", "x"),
        ("assets/sub/a b.svg", "<svg/>"),
        ("templates/secret.html", "s"),
        ("README", "r"),
    ]);
    assert_eq!(
        t.asset("app.js").unwrap().1,
        "text/javascript; charset=utf-8"
    );
    assert_eq!(t.asset("sub/a b.svg").unwrap().1, "image/svg+xml");
    for miss in [
        "../theme.toml",
        "../templates/index.html",
        "templates/index.html",
        "theme.toml",
        "/app.js",
        "./app.js",
        "sub/../app.js",
        "sub%2Fa%20b.svg",
        "README",
        "",
    ] {
        assert!(t.asset(miss).is_none(), "{miss:?}");
    }
    assert_eq!(
        t.asset_url("sub/a b.svg"),
        format!("/_theme/{}/sub/a%20b.svg", t.hash())
    );
}

#[test]
fn hash_changes_with_any_file() {
    let a = theme(&[("assets/a.css", "x")]);
    let b = theme(&[("assets/a.css", "y")]);
    let c = theme(&[("assets/a.css", "x")]);
    assert_ne!(a.hash(), b.hash());
    assert_eq!(a.hash(), c.hash());
}

#[test]
fn index_is_required() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "theme.toml", MANIFEST.as_bytes());
    write(dir.path(), "templates/single.html", b"x");
    let err = Theme::load(&ThemeSource::Dir(dir.path().into())).unwrap_err();
    assert!(
        matches!(&err, ThemeError::Invalid(p) if p.iter().any(|m| m.contains("templates/index.html"))),
        "{err}"
    );
}

#[test]
fn manifest_problems_are_reported() {
    let err = load(&[(
        "theme.toml",
        "[theme]\nname = \"Bad Name\"\nversion = \"1\"\n",
    )])
    .unwrap_err();
    assert!(matches!(err, ThemeError::Invalid(_)), "{err}");
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "templates/index.html", b"x");
    let err = Theme::load(&ThemeSource::Dir(dir.path().into())).unwrap_err();
    assert!(err.to_string().contains("theme.toml is missing"), "{err}");
}

#[test]
fn template_syntax_errors_list_file_and_line() {
    let err = load(&[
        ("templates/a.html", "line1\n{% if %}\n"),
        ("templates/partials/b.html", "x\ny\n{{ foo( }}\n"),
        ("templates/ok.html", "{{ 1 }}"),
    ])
    .unwrap_err();
    let ThemeError::Templates(errors) = err else {
        panic!("{err}")
    };
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(
        errors[0].starts_with("templates/a.html:2: syntax error"),
        "{errors:?}"
    );
    assert!(
        errors[1].starts_with("templates/partials/b.html:3: syntax error"),
        "{errors:?}"
    );
}

#[test]
fn non_utf8_template_is_rejected() {
    let dir = write_theme(&[]);
    write(dir.path(), "templates/bin.html", &[0xff, 0xfe]);
    let err = Theme::load(&ThemeSource::Dir(dir.path().into())).unwrap_err();
    assert!(
        err.to_string().contains("templates/bin.html is not UTF-8"),
        "{err}"
    );
}

#[cfg(unix)]
#[test]
fn symlinks_are_rejected() {
    let outside = tempfile::tempdir().unwrap();
    write(outside.path(), "passwd", b"root:x:0:0");
    for link in ["assets/passwd", "templates/x.html", "assets"] {
        let dir = write_theme(&[]);
        let at = dir.path().join(link);
        if at.exists() {
            std::fs::remove_dir_all(&at).unwrap();
        }
        std::fs::create_dir_all(at.parent().unwrap()).unwrap();
        let target = if link == "assets" {
            outside.path().to_path_buf()
        } else {
            outside.path().join("passwd")
        };
        std::os::unix::fs::symlink(target, &at).unwrap();
        let err = Theme::load(&ThemeSource::Dir(dir.path().into())).unwrap_err();
        assert!(
            err.to_string().contains("symlinks are not allowed"),
            "{link}: {err}"
        );
    }
    // The theme root itself must be a real directory.
    let dir = write_theme(&[]);
    let link = outside.path().join("theme-link");
    std::os::unix::fs::symlink(dir.path(), &link).unwrap();
    assert!(Theme::load(&ThemeSource::Dir(link)).is_err());
}

#[test]
fn file_count_cap() {
    let dir = write_theme(&[]);
    for i in 0..MAX_FILES {
        write(dir.path(), &format!("assets/f{i}.txt"), b"");
    }
    let err = Theme::load(&ThemeSource::Dir(dir.path().into())).unwrap_err();
    assert!(err.to_string().contains("more than 2000 files"), "{err}");
}

#[test]
fn total_size_cap() {
    let dir = write_theme(&[]);
    let chunk = vec![b'a'; 7 * 1024 * 1024];
    for i in 0..3 {
        write(dir.path(), &format!("assets/big{i}.bin"), &chunk);
    }
    let err = Theme::load(&ThemeSource::Dir(dir.path().into())).unwrap_err();
    assert!(err.to_string().contains("bytes in total"), "{err}");
}

#[test]
fn hidden_files_are_ignored() {
    let t = theme(&[("assets/.DS_Store", "x"), ("assets/a.css", "a")]);
    assert!(t.asset(".DS_Store").is_none());
    assert!(t.asset("a.css").is_some());
}

#[test]
fn theme_and_renderer_are_shareable_across_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Theme>();
    assert_send_sync::<ferrite_theme::Renderer>();
}
