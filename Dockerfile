# ── Stage 1: Build ────────────────────────────────────────────────────────────
FROM rust:1.78-slim-bookworm AS builder

WORKDIR /app

# Build dependencies (needed for sqlx TLS and openssl)
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Cache dependency compilation separately from application code.
# Copy manifests first; build a stub binary so cargo fetches and compiles deps.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && echo "" > src/lib.rs
RUN cargo build --release
RUN rm -rf src

# Copy real source and assets
COPY src ./src
COPY migrations ./migrations
COPY assets ./assets
COPY templates ./templates

# sqlx compile-time query checks require either a live DATABASE_URL or the
# pre-generated .sqlx directory. Commit .sqlx/ after running:
#   cargo sqlx prepare
# Then set SQLX_OFFLINE=true here so builds are hermetic.
ENV SQLX_OFFLINE=true

# Touch to bust the cached stub binary
RUN touch src/main.rs src/lib.rs && cargo build --release

# ── Stage 2: Runtime ──────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/target/release/clanplan ./clanplan
COPY --from=builder /app/assets ./assets
COPY --from=builder /app/migrations ./migrations

# Media storage mount point — map a host volume here in compose
RUN mkdir -p /data/media

EXPOSE 8080

CMD ["./clanplan"]
