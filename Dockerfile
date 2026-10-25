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

FROM node:24-bookworm-slim AS admin
RUN npm install -g pnpm@10.28.2
WORKDIR /src/admin
COPY admin/package.json admin/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY admin/ ./
RUN pnpm build

FROM rust:1-bookworm AS builder
WORKDIR /src
COPY . .
COPY --from=admin /src/admin/dist /src/admin/dist
RUN cargo build --release --locked -p ferrite \
    && mkdir /data-empty

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /src/target/release/ferrite /ferrite
# An empty /data owned by the nonroot user (65532), so a fresh volume is writable.
COPY --from=builder --chown=65532:65532 /data-empty /data
VOLUME /data
EXPOSE 8080
ENTRYPOINT ["/ferrite"]
CMD ["serve", "--data", "/data"]
