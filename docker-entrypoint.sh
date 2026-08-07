#!/bin/sh
# Single-process entrypoint for LRS.
# TS server manages Rust proxy as child process (auto-spawn/restart/monitor).
set -e

echo "[entrypoint] Starting LRS on :${PORT:-3000}"
exec bun run src/server.ts
