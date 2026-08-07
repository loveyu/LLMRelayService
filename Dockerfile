# ── Stage 1: Build frontend ───────────────────────────────────────────────────
FROM oven/bun:1 AS builder

WORKDIR /app

# Install dependencies (workspace-aware)
COPY package.json bun.lock bunfig.toml ./
COPY console/ai-proxy-dashboard/package.json ./console/ai-proxy-dashboard/
RUN bun install --frozen-lockfile

# Copy source and build frontend static assets
COPY . .
RUN bun run build

# ── Stage 2: Build Rust proxy ──────────────────────────────────────────────────
FROM rust:alpine AS rust-builder

RUN apk add --no-cache musl-dev

WORKDIR /app/rust-proxy

# Cache dependencies
COPY rust-proxy/Cargo.toml rust-proxy/Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && \
    cargo build --release && rm -rf src

# Build actual source
COPY rust-proxy/src ./src
RUN cargo build --release --locked && \
    strip target/release/rust-proxy

# ── Stage 3: Runtime ───────────────────────────────────────────────────────────
FROM oven/bun:1 AS runner

WORKDIR /app

# Copy TS runtime
COPY --from=builder /app/package.json /app/bun.lock /app/bunfig.toml ./
COPY --from=builder /app/node_modules ./node_modules
COPY --from=builder /app/src ./src
COPY --from=builder /app/drizzle ./drizzle
COPY --from=builder /app/dist ./dist
COPY --from=builder /app/drizzle.config.ts ./

# Copy Rust binary
COPY --from=rust-builder /app/rust-proxy/target/release/rust-proxy /usr/local/bin/rust-proxy

# Copy entrypoint script
COPY docker-entrypoint.sh /usr/local/bin/
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

ENV PORT=3000
ENV SERVER_HOST=0.0.0.0
ENV RUST_PROXY_PORT=3301
ENV RUST_PROXY_HOST=0.0.0.0
EXPOSE 3000 3301

ENTRYPOINT ["docker-entrypoint.sh"]
