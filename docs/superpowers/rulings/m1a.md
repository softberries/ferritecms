# M1a storage — controller rulings

Decisions made during subagent-driven execution of docs/superpowers/plans/2026-10-22-m1a-storage.md. Format: ruling — reason — cost if wrong.

- Ruling R1 |
- Rulings
- Ruling R1: Task 5 needs an author user before UserRepo exists (Task 7) → Task 5 implements `UserRepo::insert` + `get` early (minimal), Task 7 completes the rest of UserRepo — cost if wrong: trivial re
- Ruling R2: Postgres tests locally use FERRITE_TEST_PG_URL=postgres://ferrite@localhost:54329/ferrite_test (controller-run PG17 in scratchpad); implementers must run with it set so both backends are ex
- Ruling R3: Secret newtype (redacting Debug, no serde); User/NewUser drop serde derives — API/export use DTOs — cost if wrong: M4 export writes a DTO (planned anyway)
- Ruling R4: commit trailers use each subagent's own harness attribution (accurate model name) — cost: none
- Ruling R5: contract tests take &TestDb (store() + schema dump/tamper helpers) instead of &dyn Store — needed for migration tests — cost: none. codec.rs deferred to T4/5 (no dead code). public._sqlx_mi
- Ruling R6: SqliteRepos holds reader+writer Exec; SELECTs→reader, mutations→writer; Tx uses its transaction; reader pool read_only(true); pattern documented for T4–T10 — spec 02 §6 single writer — cost
- Ruling R7: JSON containing NUL rejected with StoreError::Invalid on BOTH backends via shared ferrite-store helper used by every JSON-writing repo method (PG jsonb can't store it) — parity anti-goal — 
- Ruling R8: text ordering parity — Postgres ORDER BY ... COLLATE "C" to match SQLite BINARY for all key/slug orderings — cost if wrong: admin lists sort byte-wise not locale-aware (acceptable, revisit 
- Ruling R9: content parent delete stays RESTRICT → StoreError::ForeignKey; core decides reparenting — cost if wrong: core must handle children before delete
- Ruling R10: editing 0001_init in place is allowed until the first release tag (nothing deployed); after a tag, migrations are immutable — cost if wrong: dev DBs re-created
- Ruling R11: multi-statement replace ops use a savepoint on the Tx path (set_for_item fixed in T7, menu put) — parity — cost if wrong: small
- Ruling R12: email uniqueness via Rust ascii-lowercased `email_key` column + plain UNIQUE on both backends (0001 edited, R10) — parity — cost if wrong: non-ASCII case variants are distinct accounts (do
- Ruling R13: shared validate_text rejects NUL (Invalid) in every caller-text write on both backends; sweep done in T7 fix round; later tasks must apply it — cost if wrong: none
- Ruling R14: RED-run requirement waived for T7 (reviewer verified R11 tests discriminate); TDD still required for T8–T10
- Ruling R15: Task 10 (scaffold removal) also sweeps ALL repos so every caller-text bind — writes AND lookups/deletes/reads — calls validate_text, giving identical Invalid on both backends; contract tes
- Ruling R16: search semantics identical on both: whitespace terms → literal AND-joined prefix terms (no operators) on both; SQLite FTS5 unicode61 remove_diacritics 0 (accent-sensitive like PG simple); 
- Ruling R17: Rust owns search normalization (NFKC, lowercase, split keeping combining marks, caps: 64 chars/term, 32 query terms, 4096 index terms); SQLite external-content FTS5 with tokenize='ascii' +
- Ruling R18: poison semantics on both backends — DB-level error in a Tx poisons it (Aborted on later calls, commit → Err(Aborted)); validation errors don't; savepoints (atomic!/nested) contain failures
- Ruling R19: I4, I5 (isolation policy), I6, M4, I2 fail-fast detection → first tasks of the M1c plan (store API additions), not this fix wave — API needs come from core services — cost if wrong: M1c st
