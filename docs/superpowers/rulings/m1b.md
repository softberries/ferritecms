# M1b media — controller rulings

Decisions made during subagent-driven execution of docs/superpowers/plans/2026-10-22-m1b-media.md.

- Rulings
- Ruling R1: T2 put_artifact returns MediaError::Config("payload too large for put_artifact; use put_stream") for ≥ 8 MiB until T3 adds delegation; T3 switches it to delegate — avoids forward dependency — cost: none
- Ruling R2: ArtifactRef.created is advisory under concurrent writers (S3 copy fallback may report true twice); the artifacts DB row via insert_if_absent is the source of truth — cost: none
- Ruling R3: preserve ICC profile via encoder set_icc_profile; strip EXIF/XMP — colour fidelity — cost: few KB per variant
- Ruling R4: keep 100 MP cap (design 03); document ~3×w×h×4 peak; M1c job runner bounds image-processing concurrency with a semaphore — cost if wrong: memory spikes under parallel uploads
- Ruling R5: fix streaming read, MIME allowlist, size cap, validation hardening, ICC cap in M1b fix wave (media API owns them); S3 attributes + copy_if_not_exists config + rustls default provider → ROADMAP M1c/M4 — cost: none
- Ruling R6: M1c-1 draft plan committed in 020f020 on this branch is intended (docs only) — cost: none
- Plan ruling P1: no WebP variant output in v1 (image crate's WebP encoder is lossless-only; lossy needs a C dependency) — JPEG q82 / PNG.
