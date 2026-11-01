# Contributing to FerriteCMS

Issues and pull requests are welcome. Before a larger change, open an
[issue](https://github.com/softberries/ferritecms/issues) to discuss it, so that nobody
spends time on something that will not be merged.

## Contributor License Agreement

FerriteCMS is offered under the AGPLv3 and under a commercial license (see
[LICENSING.md](LICENSING.md)). To make that possible, every contributor signs the
[FerriteCMS Individual Contributor License Agreement](https://gist.github.com/softberries/8138f237f3e99df962c495bb9d577c50) before a pull request
can be merged.

- **How to sign.** When you open your first pull request, CLA assistant comments on it with
  a link. Sign in with GitHub and accept the agreement; the check on the pull request then
  passes.
- **Once is enough.** The signature covers your past and future contributions. You are
  asked again only if the text of the agreement changes.
- **What it says.** You keep the copyright in your work and grant SOFTBERRIES Krzysztof
  Grajek a license to use and relicense it, including under a commercial license. In
  return, every contribution that is included stays available under the AGPLv3 or another
  OSI-approved license. Read the full text before you sign.
- **Work for an employer.** If your employer has rights in what you write, make sure you
  are allowed to contribute it under the agreement before you sign.

Plugins and themes you write yourself are not contributions to FerriteCMS: you need no
agreement for them, and [LICENSE-EXCEPTION.md](LICENSE-EXCEPTION.md) lets you license them
under any terms.

Questions about the agreement: [hello@softberries.dev](mailto:hello@softberries.dev).

## Building and testing

You need Rust (stable, through [rustup](https://rustup.rs), with the `wasm32-wasip2`
target that `rust-toolchain.toml` names), Node.js 24.18 or newer and pnpm 10.28.2. The
Postgres tests need a Postgres 15 or newer; without `FERRITE_TEST_PG_URL` they print `SKIP`
and pass.

| What | Command |
|---|---|
| Rust tests (SQLite, and Postgres when the URL is set) | `FERRITE_TEST_PG_URL=postgres://… cargo test --workspace` |
| Plugins (their own workspace) | `cargo build --manifest-path plugins/Cargo.toml --release --target wasm32-wasip2` |
| Admin lint, typecheck, unit tests, build | `pnpm -C admin install`, then `pnpm -C admin lint`, `typecheck`, `test`, `build` |
| Browser tests against the real binary | `cargo build --release -p ferrite` after the admin build, then `pnpm -C admin e2e` (see [`admin/README.md`](admin/README.md)) |
| Documentation site | `site/build.sh OUT_DIR` (see [`site/README.md`](site/README.md)) |

## Before you open a pull request

CI runs these on every pull request; run them locally first:

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
```

If you changed the admin, also run `pnpm -C admin lint`, `pnpm -C admin typecheck` and
`pnpm -C admin test`. If you changed the plugins, CI also runs fmt and clippy on
`plugins/`.

- Keep a pull request to one change, and explain what it does and why.
- Add or update tests for behaviour you change. A bug fix comes with a test that failed
  before it.
- Storage changes need a migration on both backends (SQLite and Postgres, each with a
  `.down.sql`) and contract tests on both.
- API changes regenerate the checked-in OpenAPI spec:
  `FERRITE_UPDATE_OPENAPI=1 cargo test -p ferrite-http --test api openapi`.
- The plugin API in `wit/` is versioned: a breaking change needs a new world version.
- Update the documentation in `site/content/` when you change something users see.
