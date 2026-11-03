use ferrite_store::{Phase, expand_lint, parse_directives};
use ferrite_store_tests::change_sets::*;
use ferrite_store_tests::{
    artifact_gc::*, audit::*, auth::*, autosave::*, backend_tests, backend_tests_spawning,
    batch::*, comment::*, comment_counts::*, comment_threads::*, constraints::*, content::*,
    event::*, export::*, idempotency::*, import::*, import_writer::*, job::*, job_list::*,
    locale::*, lock::*, menu::*, migration_dirs, migration_files, migrations::*, node::*,
    node_metrics::*, notify::*, oauth::*, plugin::*, plugin_failure::*, plugin_trust::*, pool::*,
    quality::*, search::*, semantics::*, site::*, taxonomy::*, tx::*, user::*, user_filters::*,
    webhooks::*,
};

backend_tests!(
    fresh_db_is_up_to_date,
    up_down_up_roundtrip,
    verdict_after_partial_down,
    verify_detects_tampered_checksum,
    min_binary_survives_older_binary,
    status_reports_pending_directives,
    migration_0006_roundtrip,
    migration_0007_roundtrip,
    migration_0008_roundtrip,
    migration_0009_roundtrip,
    migration_0010_roundtrip,
    migration_0012_roundtrip,
    migration_0013_roundtrip,
    migration_0015_roundtrip,
    migrate_up_reports_snapshot,
    migration_0014_roundtrip,
    backfill_uses_site_locale_option,
    old_style_insert_without_new_columns_works,
    slug_unique_per_locale,
    translation_unique_per_group,
    down_deletes_non_default_rows_and_restores_indexes,
);

backend_tests!(
    insert_defaults_group_to_id,
    locale_filter_on_list_count_and_file_bounds,
    media_ignores_locale_filter,
    get_by_slug_with_and_without_locale,
    same_slug_two_locales,
    group_members_batches_and_caps,
    relocate_checks_version_and_group_uniqueness,
    term_slug_per_locale,
    menu_translation_lookup,
    locale_usage_counts,
    relabel_all_and_blank,
    relabel_media_moves_only_media,
    fill_groups_sets_null_groups,
    search_locale_filter,
);

backend_tests!(
    oauth_client_upsert_get_touch,
    oauth_request_is_taken_once,
    oauth_code_expires,
    grant_rotation_keeps_previous_hash,
    deleting_grant_deletes_its_tokens,
    personal_token_counts_ignore_oauth_tokens,
    prune_removes_expired_oauth_rows,
    deleting_user_cascades_grants,
    delete_codes_for_user_leaves_used_and_foreign_codes,
    export_leaves_out_oauth_tokens,
    import_refuses_oauth_token_rows,
);

backend_tests!(
    owner_round_trip_with_null_owner,
    owner_outlives_plugin_row,
    owners_signed_by_finds_current_signer,
    key_upsert_and_revoke,
    public_key_unique,
    registry_row_replaced_by_url,
    registry_checked_keeps_index,
    export_catalogue_lists_owner_and_key_tables,
    key_delete_and_owner_signer_key,
);

backend_tests!(
    node_metrics_insert_and_series_sum_across_nodes,
    series_p95_from_combined_histogram,
    prune_older_than_7_days,
);

backend_tests!(
    import_run_roundtrip_and_running,
    fence_true_only_for_matching_running_token,
    abandon_stale_only_when_still_stale,
    abandon_stale_for_job_spares_live_jobs,
    an_expired_lease_does_not_keep_a_run_live,
    import_map_upsert_and_get,
    import_map_list_pages_by_wp_id,
    content_insert_keeps_explicit_timestamps,
    import_run_job_link_and_step,
    import_run_report_hash,
    import_runs_page_cursor,
    artifact_kind_import_round_trips,
);

backend_tests_spawning!(fence_serialises_with_takeover);
backend_tests_spawning!(abandon_stale_rechecks_after_fence);

backend_tests!(
    autosave_upsert_get_delete,
    autosave_cascades_with_item_and_user,
    autosave_keeps_seo,
);

backend_tests!(
    options_crud,
    options_list_prefix,
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
    set_updated_at_keeps_version,
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
    list_readable_by,
    list_readable_by_hides_future_dated,
    count_matches_list,
    sitemap_file_bounds,
    get_many_order_and_missing,
    slugs_with_prefix_matches,
    slugs_with_prefix_limit_1000,
    content_query_kinds,
    media_kind_filters_list_and_count,
);

backend_tests!(
    taxonomies_seeded_and_upsert,
    taxonomy_delete,
    type_and_taxonomy_insert,
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
    redirect_add_hits,
    redirects_list_paginates_in_binary_order,
    redirect_search_escapes_wildcards,
    redirects_list_by_target,
    text_writes_reject_nul,
    text_lookups_reject_nul,
);

backend_tests!(
    users_insert_get,
    email_case_insensitive_unique,
    unicode_login_and_slug_exact,
    users_update_and_list,
    user_list_filters,
    session_public_id_roundtrip,
    email_folding_is_ascii_only,
    set_totp_sets_secret_and_resets_step,
    update_keeps_totp_last_step,
    user_delete_restricted_by_content_then_ok_after_reassign,
    count_active_admins_ignores_disabled_and_other_roles,
    list_active_admins_filters,
);

backend_tests!(
    artifact_insert_if_absent,
    artifact_list_page_is_ordered_and_paged,
    existing_returns_present_subset,
    migration_0011_roundtrip,
    delete_if_orphaned_keeps_each_reference_kind,
    mark_orphans_marks_unreferenced_and_clears_referenced,
    insert_if_absent_clears_orphan_mark,
    delete_if_orphaned_respects_before_refs_and_protected,
    media_insert_get_update_variants,
    media_find_by_artifact,
    media_variants_backfill,
    media_lookup_uses_indexes,
    media_sanitized_roundtrip,
    plugins_upsert_list_enable,
    plugins_action_subscribers,
    retained_plugin_markers,
    plugin_kv_crud_and_usage,
    plugin_kv_delete_all_in_batches,
    themes_single_active,
    kv_quota_semantics,
    plugin_failures_add_sum_clear_prune,
    plugin_auto_disable_is_one_transition,
);

backend_tests!(
    events_append_since_max,
    events_reject_nul,
    events_in_tx_only_visible_after_commit,
    events_prune,
    events_prune_in_batches,
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
    search_readable_by,
    search_readable_by_hides_future_dated,
    search_ties_break_by_id_descending,
    cursor_kinds_are_not_interchangeable,
);

backend_tests!(batch_loaders_match_single_gets);

backend_tests!(
    jobs_enqueue_and_get_roundtrip,
    jobs_unique_key_dedupes_in_any_status,
    jobs_claim_respects_run_at_priority_and_kinds,
    jobs_expired_lease_is_reclaimed,
    jobs_claim_orders_queued_and_expired_leases_together,
    jobs_expired_lease_at_max_attempts_goes_dead,
    jobs_stale_lease_holder_cannot_write,
    jobs_fail_backoff_then_dead,
    jobs_release_without_attempt,
    jobs_release_final_attempt_goes_dead,
    jobs_error_text_is_sanitised,
    jobs_validation_does_not_poison,
    jobs_prune_by_status_age,
    jobs_prune_in_batches,
    jobs_tolerate_unknown_columns,
    job_list_filters_and_pages,
    retry_dead_only,
    failure_buckets_since_newest_first,
);

backend_tests!(
    lock_is_exclusive_and_released,
    lock_excludes_peer_store,
    lock_released_when_holder_dies,
    pg_lock_wait_times_out,
    pg_lock_wait_cancel_does_not_leak,
    lock_names_are_validated,
    nodes_register_heartbeat_list_prune,
    nodes_prune_in_batches,
    nodes_list_keeps_newest_heartbeats,
    pool_stats_count_used_and_idle,
);

backend_tests!(
    sessions_roundtrip_touch_delete,
    sessions_prune_by_expiry_and_idle,
    api_tokens_unique_hash_and_owner_scoped_delete,
    api_tokens_prune_expired,
    rate_limit_counts_within_window_and_resets,
    rate_limit_window_never_moves_back,
    audit_append_and_list_newest_first,
    audit_filters,
    auth_text_binds_reject_nul,
);

backend_tests!(
    comment_crud_and_status,
    comment_list_filters_and_pages,
    comment_counts_per_status_and_item,
    thread_pages_cover_every_comment,
    approved_page_keeps_every_root,
    short_comment_page_under_tight_budget,
    root_position_of_reply_is_its_root,
    approved_totals_counts_all,
    has_approved_from_matches_exact_lowercase,
    idempotency_prune_in_batches,
    comment_list_uses_indexes,
    idempotency_prune_uses_indexes,
);

backend_tests_spawning!(
    lock_waits_then_busy,
    pg_lock_wait_wakes_on_release,
    pg_lock_wait_opens_one_connection,
    concurrent_writers,
    events_concurrent_appends_unique_seq,
    kv_quota_never_exceeded,
    jobs_claim_is_exclusive_under_concurrency,
    jobs_concurrent_enqueue_same_key,
    prune_spares_a_retried_job,
    plugin_auto_disable_concurrent_single_transition,
    plugin_get_for_update_locks_the_row,
    media_get_for_update_locks_the_row,
    rate_limit_is_atomic_under_concurrency,
    totp_step_only_moves_forward,
    user_get_for_update_locks_the_row,
    idempotency_begin_states,
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
                let down = name.replace(".up.sql", ".down.sql");
                let down = std::fs::read_to_string(dir.join(down)).unwrap();
                let problems = expand_lint(&sql, &down);
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

backend_tests!(
    catalogue_matches_live_schema,
    catalogue_is_fk_ordered,
    artifact_foreign_keys_are_gc_references,
    export_reader_encodes_neutrally,
    export_reader_pages_by_key,
    export_reader_sees_one_snapshot,
    export_reader_drop_releases,
    import_writer_round_trip,
    import_writer_rollback_leaves_nothing,
    import_writer_self_refs_any_order,
    import_writer_site_id_batches_refusals,
);

backend_tests!(
    migration_0016_roundtrip,
    endpoint_crud_and_cascade,
    active_for_emit_needs_enabled_and_install_id,
    record_attempt_is_single_use,
    set_status_only_from_pending,
    deliveries_page_newest_first,
    delivery_counts_by_status_since,
    prune_keeps_events_with_pending_deliveries,
    nul_in_text_is_invalid,
    fail_stale_pending_only_old_pending,
);

backend_tests!(
    migration_0018_roundtrip,
    extend_open_returns_consecutive_seqs,
    idle_set_is_not_extended,
    full_set_is_not_extended,
    open_closes_the_stale_set_and_starts_a_new_one,
    images_round_trip_bytes_and_hashes,
    list_is_newest_first_with_cursor_and_filters,
    reverted_counts_in_list,
    deleting_user_nulls_user_and_reverted_by,
    prune_deletes_old_sets_and_their_changes,
);

backend_tests_spawning!(concurrent_open_meets_in_one_set);

backend_tests!(
    migration_0019_roundtrip,
    quality_gate_defaults_off_and_round_trips,
    suppression_upsert_and_delete,
    suppressions_cascade_with_item,
    published_page_skips_drafts_media_and_trash,
    last_revision_at_groups_per_item,
    findings_page_filters_and_cursor,
    findings_cascade_with_run_and_item,
    delete_findings_except_keeps_one_run,
    abandon_running_marks_old_runs,
    link_cache_put_get_prune,
);
