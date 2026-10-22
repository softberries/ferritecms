use ferrite_store::{Phase, expand_lint, parse_directives};
use ferrite_store_tests::{backend_tests, migration_dirs, migration_files, migrations::*};

backend_tests!(
    fresh_db_is_up_to_date,
    up_down_up_roundtrip,
    verdict_after_partial_down,
    verify_detects_tampered_checksum,
    min_binary_survives_older_binary,
);

#[test]
fn migration_sets_match() {
    let [sqlite, pg] = migration_dirs();
    let names = migration_files(&sqlite);
    assert!(!names.is_empty());
    assert_eq!(names, migration_files(&pg));
    for name in &names {
        let (stem, kind) = name.rsplit_once('.').unwrap().0.rsplit_once('.').unwrap();
        assert!(
            kind == "up" || kind == "down",
            "{name}: not NNNN_name.up|down.sql"
        );
        let (version, _) = stem.split_once('_').unwrap();
        assert!(
            version.len() == 4 && version.bytes().all(|b| b.is_ascii_digit()),
            "{name}"
        );
        let other = if kind == "up" { "down" } else { "up" };
        assert!(
            names.contains(&format!("{stem}.{other}.sql")),
            "{name} has no {other}"
        );
    }
}

#[test]
fn expand_lint_passes() {
    for dir in migration_dirs() {
        for name in migration_files(&dir)
            .iter()
            .filter(|n| n.ends_with(".up.sql"))
        {
            let sql = std::fs::read_to_string(dir.join(name)).unwrap();
            if parse_directives(&sql).unwrap().phase == Phase::Expand {
                let problems = expand_lint(&sql);
                assert!(
                    problems.is_empty(),
                    "{}: {problems:?}",
                    dir.join(name).display()
                );
            }
        }
    }
}
