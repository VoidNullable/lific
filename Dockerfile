# You do not need Docker to run Lific. The project ships as a single binary
# with SQLite bundled (see the README for install options). This Dockerfile
# exists because MCP directory indexers (Glama and friends) build servers from
# a Dockerfile to verify them before listing; without one, Lific is invisible
# in their search. It also works fine if a container is genuinely how you want
# to deploy.
#
#   docker build -t lific .
#   docker run -p 3456:3456 -v lific-data:/data lific
#
# The database lives at /data/lific.db; mount a volume there to persist it.

# Stage 1: web UI, embedded into the binary via rust-embed.
# vite reads ../Cargo.toml for the version, so it's copied alongside.
FROM oven/bun:1 AS web
WORKDIR /src/web
COPY web/package.json web/bun.lock ./
RUN bun install --frozen-lockfile
COPY Cargo.toml /src/Cargo.toml
COPY web/ ./
RUN bun run build

# Stage 2: the binary. rustls + bundled SQLite, so no OpenSSL or sqlite dev
# packages are needed; the stock slim image's C toolchain is enough. The
# Debian release here MUST match the distroless runtime below, or the binary
# links a newer glibc than the runtime ships.
FROM rust:1-slim-trixie AS build
WORKDIR /src
COPY . .
COPY --from=web /src/web/dist ./web/dist
RUN cargo build --release --locked
# Pre-create the data dir with the runtime UID; distroless has no shell to
# mkdir/chown with, and a VOLUME dir created at run time would be root-owned.
RUN mkdir /data && chown 65532:65532 /data

# Stage 3: runtime. distroless/cc = glibc + CA certs + nothing else.
FROM gcr.io/distroless/cc-debian13:nonroot
COPY --from=build /src/target/release/lific /usr/local/bin/lific
COPY --from=build --chown=65532:65532 /data /data
VOLUME /data
EXPOSE 3456
ENTRYPOINT ["/usr/local/bin/lific", "--db", "/data/lific.db"]
CMD ["start"]
