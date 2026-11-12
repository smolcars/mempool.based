# syntax=docker/dockerfile:1
# The mempool.based server and its built frontend in one image. docs/DEPLOY.md explains it.
# Base images are multi-arch (linux/amd64, linux/arm64) and pinned by digest.

# The Rust tag must match rust-toolchain.toml. That file is not copied in: rustup would
# download rustfmt and clippy, which a release build does not need.
ARG RUST_IMAGE=docker.io/library/rust:1.99.1-slim-trixie@sha256:f6c4a35d21dbb3c89d7a8d056c193e94fc09037f2b58dd858b09c39a47dd02d7
ARG BUN_IMAGE=docker.io/oven/bun:1.4.4-slim@sha256:1846adf22496bd711036aae8be147e57bdebbaea2e5264e3e0b5af1db187d9a4
# Debian 13 like the Rust image, so the binary finds the glibc it was linked against.
ARG RUNTIME_IMAGE=gcr.io/distroless/cc-debian13:nonroot@sha256:cc66d222f0a7ec996b78a99fe76a969bc80b539a5016ad866c14d1949bc6d0ab

# The frontend is the same on every architecture, so it is built once, natively.
FROM --platform=$BUILDPLATFORM ${BUN_IMAGE} AS web
WORKDIR /src/web
COPY web/package.json web/bun.lock ./
RUN --mount=type=cache,target=/root/.bun/install/cache bun install --frozen-lockfile
COPY web/ ./
RUN bun run build

FROM ${RUST_IMAGE} AS server
ARG TARGETPLATFORM
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
# Cached target/ and registry: a one-file change recompiles one crate, not the dependency tree.
RUN --mount=type=cache,id=cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-target-$TARGETPLATFORM,target=/src/target \
    cargo build --release --locked -p mempool-based \
    && cp target/release/mempool-based /mempool-based

FROM ${RUNTIME_IMAGE}
COPY --from=server /mempool-based /usr/local/bin/mempool-based
COPY --from=web /src/web/dist /srv/web
ENV LISTEN_ADDR=0.0.0.0:8080 \
    WEB_DIR=/srv/web
EXPOSE 8080
# distroless "nonroot": uid and gid 65532. Nothing in the image is writable by it.
USER 65532:65532
HEALTHCHECK --interval=30s --timeout=6s --start-period=10s --start-interval=1s \
    CMD ["/usr/local/bin/mempool-based", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/mempool-based"]
