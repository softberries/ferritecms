# syntax=docker/dockerfile:1
# FerriteCMS image (design 09 §7): one static-ish binary on distroless, non-root.
#
#   docker build -t ferrite .
#   docker run -v ferrite-data:/data ferrite init --data /data --url http://localhost:8080
#
# The `admin` stage builds the admin SPA (`admin/dist`), which the Rust build embeds
# (without it the binary serves the admin placeholder). The builder is the current stable
# Rust (rust-toolchain.toml pins `channel = "stable"`), on Debian 12 so the glibc matches
# the runtime image.
#
# Every base image is pinned as `name:tag@sha256:<multi-arch index digest>` (M4-2 P37), so
# the base layers are fixed (the Rust toolchain still follows `stable`, which rustup
# installs at build time per rust-toolchain.toml); the tag only says what the digest was
# taken from (2026-10-30, `docker buildx imagetools inspect`). Bump the digests by hand
# to pick up new base releases. The distroless runtime carries the CA certificates
# (`/etc/ssl/certs/ca-certificates.crt`) that outgoing HTTPS (S3 media, plugin `http`,
# `self-update`) verifies against.

FROM node:24-bookworm-slim@sha256:c86ecead9e55a5210ac1727de38a4ca840bf03dcaf34dc9749addbf383b1920c AS admin
RUN npm install -g pnpm@10.28.2
WORKDIR /src/admin
COPY admin/package.json admin/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY admin/ ./
RUN pnpm build

FROM rust:1-bookworm@sha256:9181b56c2a21ab1d5e73cffe04fc762426d30aa7fac3c91141dd033f49a7fc8b AS builder
WORKDIR /src
COPY . .
COPY --from=admin /src/admin/dist /src/admin/dist
RUN cargo build --release --locked -p ferrite \
    && mkdir /data-empty

FROM gcr.io/distroless/cc-debian12:nonroot@sha256:9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f
COPY --from=builder /src/target/release/ferrite /ferrite
# An empty /data owned by the nonroot user (65532), so a fresh volume is writable.
COPY --from=builder --chown=65532:65532 /data-empty /data
VOLUME /data
EXPOSE 8080
ENTRYPOINT ["/ferrite"]
CMD ["serve", "--data", "/data"]
