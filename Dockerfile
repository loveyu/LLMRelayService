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

# ── Stage 2: Runtime ───────────────────────────────────────────────────────────
FROM oven/bun:1 AS runner

WORKDIR /app

# curl provides stable HTTP/HTTPS/SOCKS proxy support under the Bun runtime.
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates curl \
  && rm -rf /var/lib/apt/lists/*

# Copy TS runtime
COPY --from=builder /app/package.json /app/bun.lock /app/bunfig.toml ./
COPY --from=builder /app/node_modules ./node_modules
COPY --from=builder /app/src ./src
COPY --from=builder /app/drizzle ./drizzle
COPY --from=builder /app/dist ./dist
COPY --from=builder /app/drizzle.config.ts ./

# Copy pre-built Rust binary (built in CI)
COPY rust-proxy/target/release/rust-proxy /usr/local/bin/rust-proxy
RUN chmod +x /usr/local/bin/rust-proxy

# Copy entrypoint script
COPY docker-entrypoint.sh /usr/local/bin/
RUN chmod +x /usr/local/bin/docker-entrypoint.sh

ENV PORT=3000
ENV SERVER_HOST=0.0.0.0
ENV RUST_PROXY_PORT=3301
ENV RUST_PROXY_HOST=0.0.0.0
EXPOSE 3000 3301

ENTRYPOINT ["docker-entrypoint.sh"]
