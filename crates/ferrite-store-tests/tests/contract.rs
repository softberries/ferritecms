use ferrite_store::{Phase, expand_lint, parse_directives};
use ferrite_store_tests::{
    backend_tests, backend_tests_spawning, batch::*, constraints::*, content::*, event::*, job::*,
    menu::*, migration_dirs, migration_files, migrations::*, notify::*, plugin::*, search::*,
    semantics::*, site::*, taxonomy::*, tx::*, user::*,
};

backend_tests!(
    fresh_db_is_up_to_date,
    up_down_up_roundtrip,
    verdict_after_partial_down,
    verify_detects_tampered_checksum,
    min_binary_survives_older_binary,
    status_reports_pending_directives,
);

backend_tests!(
    options_crud,
    tx_commit_visible,
    tx_rollback_invisible,
    tx_dropped_is_rolled_back,
    tx_dropped_at_test_end,
    read_sees_committed_after_commit,
    options_reject_nul,
    options_number_roundtrip,
    autoloaded_order_is_binary,
    schema_meta_roundtrip,
);

backend_tests!(
    unique_violations_name_the_constraint,
    foreign_key_violations_have_empty_constraint,
);

backend_tests!(
    missing_rows_update_not_found_delete_ok,
    revisions_list_checks_limit,
    redirects_upsert_returns_stored_and_hit_time,
    close_fails_later_calls,
);

backend_tests!(
    tx_unique_violation_poisons,
    tx_foreign_key_poisons,
    tx_validation_error_does_not_poison,
    tx_nested_failure_keeps_outer,
    tx_nested_commit_rollback_drop,
    sqlite_begin_while_tx_open_is_busy,
    nested_open_error_variant,
    outer_drop_finishes_nested,
    outer_rollback_finishes_nested,
    outer_drop_with_parked_nested,
    outer_commit_with_nested_open_rolls_back,
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
    list_filters_ids_exclude_dates,
    count_matches_list,
    get_many_order_and_missing,
    slugs_with_prefix_matches,
    slugs_with_prefix_limit_1000,
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
    redirects_list_paginates_in_binary_order,
    redirects_list_by_target,
    text_writes_reject_nul,
    text_lookups_reject_nul,
);

backend_tests!(
    users_insert_get,
    email_case_insensitive_unique,
    unicode_login_and_slug_exact,
    users_update_and_list,
    email_folding_is_ascii_only,
);

backend_tests!(
    artifact_insert_if_absent,
    media_insert_get_update_variants,
    media_find_by_artifact,
    media_variants_backfill,
    media_lookup_uses_indexes,
    plugins_upsert_list_enable,
    plugin_kv_crud_and_usage,
    themes_single_active,
    kv_quota_semantics,
);

backend_tests!(
    events_append_since_max,
    events_reject_nul,
    events_in_tx_only_visible_after_commit,
    events_prune,
    events_min_seq,
);

backend_tests!(
    subscribe_is_none_on_sqlite,
    notify_events_only_after_commit,
    notify_jobs_on_enqueue,
    subscribe_reports_reconnect,
    notify_is_isolated_per_schema,
);

backend_tests!(
    search_finds_title_and_body,
    search_reindex_replaces,
    search_remove_and_cascade,
    search_handles_operators_safely,
    search_rejects_nul_and_bad_limits,
    search_semantics_identical,
    search_filters_status_kind_and_pages,
    search_ties_break_by_id_descending,
    cursor_kinds_are_not_interchangeable,
);

backend_tests!(batch_loaders_match_single_gets);

backend_tests!(
    jobs_enqueue_and_get_roundtrip,
    jobs_unique_key_dedupes_in_any_status,
    jobs_claim_respects_run_at_priority_and_kinds,
    jobs_expired_lease_is_reclaimed,
    jobs_expired_lease_at_max_attempts_goes_dead,
    jobs_stale_lease_holder_cannot_write,
    jobs_fail_backoff_then_dead,
    jobs_release_without_attempt,
    jobs_release_final_attempt_goes_dead,
    jobs_error_text_is_sanitised,
    jobs_validation_does_not_poison,
    jobs_prune_by_status_age,
    jobs_tolerate_unknown_columns,
);

backend_tests_spawning!(
    concurrent_writers,
    events_concurrent_appends_unique_seq,
    kv_quota_never_exceeded,
    jobs_claim_is_exclusive_under_concurrency,
    jobs_concurrent_enqueue_same_key,
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

/// Every repository is implemented: the only `Unsupported(` left in the backends' sources
/// is Postgres `snapshot` (it would need pg_dump on the host) and the Postgres server
/// version check.
#[test]
fn no_unsupported_left() {
    fn walk(dir: &std::path::Path, hits: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, hits);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                for (n, line) in text.lines().enumerate() {
                    if line.contains("Unsupported(") {
                        hits.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                    }
                }
            }
        }
    }
    let mut hits = Vec::new();
    for dir in migration_dirs() {
        walk(&dir.parent().unwrap().join("src"), &mut hits);
    }
    let expected = [
        "Unsupported(\"snapshot\")",
        "Unsupported(\"postgres >= 15 required\")",
    ];
    assert!(
        hits.iter().all(|h| h.contains("ferrite-store-postgres")
            && expected.iter().any(|e| h.contains(e))),
        "{hits:#?}"
    );
    assert!(hits.iter().any(|h| h.contains(expected[0])), "{hits:#?}");
}
