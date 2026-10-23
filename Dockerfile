# syntax=docker/dockerfile:1
# FerriteCMS image (design 09 §7): one static-ish binary on distroless, non-root.
#
#   docker build -t ferrite .
#   docker run -v ferrite-data:/data ferrite init --data /data --url http://localhost:8080
#
# The builder is the current stable Rust (rust-toolchain.toml pins `channel = "stable"`),
# on Debian 12 so the glibc matches the runtime image.

FROM rust:1-bookworm AS builder
WORKDIR /src
COPY . .
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
