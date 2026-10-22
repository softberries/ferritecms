use ferrite_store::{Phase, expand_lint, parse_directives};
use ferrite_store_tests::{
    backend_tests, backend_tests_spawning, content::*, menu::*, migration_dirs, migration_files,
    migrations::*, site::*, taxonomy::*, user::*,
};

backend_tests!(
    fresh_db_is_up_to_date,
    up_down_up_roundtrip,
    verdict_after_partial_down,
    verify_detects_tampered_checksum,
    min_binary_survives_older_binary,
);

backend_tests!(
    options_crud,
    tx_commit_visible,
    tx_rollback_invisible,
    tx_dropped_is_rolled_back,
    read_sees_committed_after_commit,
    options_reject_nul,
    options_number_roundtrip,
    autoloaded_order_is_binary,
    schema_meta_roundtrip,
);

backend_tests!(
    types_seeded_and_upsert,
    content_insert_get_roundtrip,
    slug_unique_per_type_and_parent,
    unknown_references_are_foreign_keys,
    get_by_slug_root_and_child,
    optimistic_lock_race,
    update_missing_is_not_found,
    list_filters_and_orders,
    list_pagination_covers_every_order,
    list_pagination_with_null_published,
    due_scheduled_returns_only_due,
    meta_crud,
    delete_cascades_meta,
    delete_with_children_is_foreign_key,
    content_rejects_nul_json,
    list_pagination_title_asc_mixed_case_unicode,
    revisions_newest_first_and_prune,
);

backend_tests!(
    taxonomies_seeded_and_upsert,
    terms_crud_and_hierarchy,
    deleting_term_with_children_is_foreign_key,
    term_links_replace_and_order,
    deleting_term_removes_links,
    content_list_by_term,
    set_for_item_failure_inside_tx_keeps_links_and_tx,
);

backend_tests!(
    menu_put_replaces_tree,
    menu_rejects_bad_parents,
    menu_put_failure_inside_tx_keeps_tree_and_tx,
);

backend_tests!(
    redirects_upsert_get_hit,
    redirects_list_paginates_in_binary_order
);

backend_tests!(
    users_insert_get,
    email_case_insensitive_unique,
    unicode_login_and_slug_exact,
    users_update_and_list,
    email_folding_is_ascii_only,
);

backend_tests_spawning!(concurrent_writers);

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
