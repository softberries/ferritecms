+++
description = "Install FerriteCMS from a signed release binary, the Docker image, or build it from source."
+++

# Install

FerriteCMS is one program, `ferrite`. There are three ways to get it: a release binary
(below), the Docker image, or a build from source. Releases are listed on
[GitHub](https://github.com/softberries/ferritecms/releases).

## Release binaries

Each release carries one archive per platform, named
`ferrite-<version>-<target>.tar.gz`, for these targets:

| Target | Notes |
|---|---|
| `x86_64-unknown-linux-gnu` | needs glibc 2.35 or newer |
| `aarch64-unknown-linux-gnu` | needs glibc 2.35 or newer |
| `x86_64-unknown-linux-musl` | static; needs the system's CA certificates |
| `aarch64-unknown-linux-musl` | static; needs the system's CA certificates |
| `aarch64-apple-darwin` | macOS on Apple silicon |
| `x86_64-apple-darwin` | macOS on Intel |

The release also carries `ferrite-plugins-<version>.tar.gz` (the three example plugins),
`SHA256SUMS` and its minisign signature `SHA256SUMS.minisig`.

### Check a download

`SHA256SUMS` lists the SHA-256 of every archive and is signed with the project's release
key. Check the signature first, then the checksum. `release.pub` is the file
`crates/ferrite/release.pub` of the repository at the release's tag.

```sh
VERSION=X.Y.Z
TARGET=x86_64-unknown-linux-gnu
BASE=https://github.com/softberries/ferritecms/releases/download/v$VERSION
curl -LO $BASE/ferrite-$VERSION-$TARGET.tar.gz
curl -LO $BASE/SHA256SUMS
curl -LO $BASE/SHA256SUMS.minisig
minisign -Vm SHA256SUMS -p release.pub
sha256sum --check --ignore-missing SHA256SUMS
tar -xzf ferrite-$VERSION-$TARGET.tar.gz
```

On macOS, use `shasum -a 256 --check --ignore-missing SHA256SUMS` instead of `sha256sum`.
The archive unpacks to a folder `ferrite-<version>-<target>/` that holds the `ferrite`
binary, a `README.md` and the license files.

The binary embeds `release.pub` when it is built, so a binary built from source holds
the same key as a release binary and `ferrite self-update` checks releases with it too.

## Docker image

Each release also publishes an image for `linux/amd64` and `linux/arm64` at
`ghcr.io/softberries/ferritecms`, tagged with the version (for example `0.1.0`), with
`0.1` when it is the newest release of that minor line, and with `latest` when it is the
newest stable release. `docker build -t ferrite .` in the repository builds the same image
from source.

The image holds only the binary (`/ferrite`) on a distroless base, runs as a non-root
user, keeps the site in the volume `/data` and starts `ferrite serve --data /data`. Create
the site in the volume first, then start it. Inside a container the server must listen on
all addresses, so set `FERRITE_SERVER__LISTEN`:

```sh
IMAGE=ghcr.io/softberries/ferritecms:0.1.0
docker run --rm -v ferrite-data:/data $IMAGE init --data /data --url http://localhost:8080
docker run --rm -v ferrite-data:/data $IMAGE user create --data /data --login me --email me@example.com
docker run -d --name ferrite -p 8080:8080 -v ferrite-data:/data \
  -e FERRITE_SERVER__LISTEN=0.0.0.0:8080 $IMAGE
```

`ferrite self-update` does not apply in a container: deploy a new image instead.

## Build from source

You need:

- Rust through [rustup](https://rustup.rs). The repository's `rust-toolchain.toml` names
  the stable toolchain (plus the `wasm32-wasip2` target, used to build plugins).
- Node.js 24.18 or newer and pnpm 10.28.2, to build the admin. Without the admin build the
  binary still works, but `/admin/` shows a page that says the admin was not built.

```sh
git clone https://github.com/softberries/ferritecms.git
cd ferritecms
pnpm -C admin install
pnpm -C admin build
cargo build --release --locked -p ferrite
./target/release/ferrite --version
```

The admin is compiled into the binary, so build it before `cargo build`. The binary is
`target/release/ferrite`; copy it anywhere on your `PATH`.

## Next

[Create your first site](02-first-site.md).
