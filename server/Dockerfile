# syntax=docker/dockerfile:1

# ---- builder ---------------------------------------------------------------
# The whole workspace (not just server/) is copied into the build context
# because z3rno-server depends on z3rno-engine via a workspace path
# dependency in Cargo.toml -- Cargo needs every member's manifest present to
# resolve the workspace at all, even though we only build the server binary.
# Pinned to bookworm explicitly (not just `rust:1-slim`) so the builder's
# glibc matches the debian:bookworm-slim runtime stage below -- a newer
# builder base (e.g. trixie) links against a glibc the runtime doesn't have.
FROM rust:1-slim-bookworm AS builder
WORKDIR /build

RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

COPY . .

RUN cargo build --release --bin z3rno-server -p z3rno-server

# ---- runtime ----------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --uid 10001 --home-dir /home/z3rno z3rno

COPY --from=builder /build/target/release/z3rno-server /usr/local/bin/z3rno-server

USER z3rno
WORKDIR /home/z3rno

EXPOSE 8080

# Z3RNO_JWT_SECRET has no default in the binary itself (see
# server/src/main.rs -- it's a required clap arg, the server refuses to
# start without one rather than falling back to an insecure default). This
# image deliberately does not bake in a placeholder secret either: an
# image with a hardcoded JWT secret would be quietly insecure-by-default
# for anyone who forgets to override it. Real usage must supply one:
#
#   docker run -e Z3RNO_JWT_SECRET=<a-real-random-secret> \
#     -p 8080:8080 ghcr.io/the-ai-project-co/z3rno-server
#
# Omitting it makes the container fail fast with a clear "missing required
# argument" error instead of starting up insecurely.
ENTRYPOINT ["/usr/local/bin/z3rno-server"]
