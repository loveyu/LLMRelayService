/**
 * Rust proxy process manager. TS manages the Rust child process lifecycle:
 *  - Spawn on startup
 *  - Auto-restart on crash (max 5 times, 3s delay)
 *  - Health check via HTTP /health
 *  - Expose status for console UI
 */

import { spawn, type Subprocess } from 'bun';

const RUST_HOST = process.env.RUST_PROXY_HOST || '127.0.0.1';
const RUST_PORT = parseInt(process.env.RUST_PROXY_PORT || '3301', 10);
const RUST_HEALTH_URL = `http://${RUST_HOST}:${RUST_PORT}/health`;
const MAX_RESTARTS = 5;
const RESTART_DELAY_MS = 3000;
const HEALTH_CHECK_INTERVAL_MS = 15000;

let _proc: Subprocess | null = null;
let _startedAt: number = 0;
let _restartCount: number = 0;
let _lastHealth: { ok: boolean; at: number; error?: string } = { ok: false, at: 0 };
let _cleaningUp = false;

// ── Public API ──────────────────────────────────────────────────────────────

export interface RustProxyStatus {
    running: boolean;
    pid: number | null;
    startedAt: number;
    uptimeMs: number;
    restartCount: number;
    health: { ok: boolean; at: number; error?: string };
    host: string;
    port: number;
    bin: string;
    realpath: string | null;
}

export function getStatus(): RustProxyStatus {
    const bin = rustBinPath();
    let realpath: string | null = null;
    try {
        realpath = require('node:fs').realpathSync(bin);
    } catch {}
    return {
        running: _proc?.exitCode === null,
        pid: _proc?.pid ?? null,
        startedAt: _startedAt,
        uptimeMs: _startedAt ? Date.now() - _startedAt : 0,
        restartCount: _restartCount,
        health: { ..._lastHealth },
        host: RUST_HOST,
        port: RUST_PORT,
        bin,
        realpath,
    };
}

/** Start the Rust proxy (call once on TS startup). */
export function startRustProxy(): void {
    if (_cleaningUp) return;
    doStart();
    startHealthCheckLoop();
}

/** Kill and restart the Rust proxy. */
export async function restartRustProxy(): Promise<{ ok: boolean; error?: string }> {
    if (_cleaningUp) return { ok: false, error: 'Shutting down' };

    if (_proc && _proc.exitCode === null) {
        _proc.kill('SIGTERM');
        // Wait up to 5s for graceful shutdown
        for (let i = 0; i < 50; i++) {
            if (_proc.exitCode !== null) break;
            await new Promise(r => setTimeout(r, 100));
        }
        if (_proc.exitCode === null) {
            _proc.kill('SIGKILL');
        }
    }

    _restartCount++;
    _startedAt = 0;
    doStart();
    return { ok: true };
}

/** Graceful shutdown — called on TS SIGTERM. */
export async function stopRustProxy(): Promise<void> {
    _cleaningUp = true;
    if (_proc && _proc.exitCode === null) {
        _proc.kill('SIGTERM');
        // Brief wait
        await new Promise(r => setTimeout(r, 500));
    }
}

// ── Internal ───────────────────────────────────────────────────────────────

function rustBinPath(): string {
    // Prefer env override, then repo build, then docker path
    if (process.env.RUST_PROXY_BIN) return process.env.RUST_PROXY_BIN;

    const repoDebug = `${import.meta.dir}/../rust-proxy/target/debug/rust-proxy`;
    const { existsSync } = require('node:fs');
    if (existsSync(repoDebug)) return repoDebug;

    const repoRelease = `${import.meta.dir}/../rust-proxy/target/release/rust-proxy`;
    if (existsSync(repoRelease)) return repoRelease;

    // Docker / system install
    return '/usr/local/bin/rust-proxy';
}

function doStart(): void {
    // Clean up stale IPC socket
    const sockPath = process.env.LRS_IPC_SOCKET || '/tmp/lrs-ipc.sock';
    try { require('node:fs').unlinkSync(sockPath); } catch {}

    const bin = rustBinPath();
    console.log(`[rust-proc] Starting ${bin} on ${RUST_HOST}:${RUST_PORT}`);

    _proc = spawn({
        cmd: [bin],
        stdout: 'pipe',
        stderr: 'pipe',
        stdin: 'ignore',
        env: {
            ...process.env,
            RUST_PROXY_HOST: RUST_HOST,
            RUST_PROXY_PORT: String(RUST_PORT),
            // Pass GATEWAY_API_KEY if set
            ...(process.env.GATEWAY_API_KEY ? { GATEWAY_API_KEY: process.env.GATEWAY_API_KEY } : {}),
        },
    });

    _startedAt = Date.now();

    // Pipe stdout/stderr with prefix
    pipeStream(_proc.stdout, 'rust');
    pipeStream(_proc.stderr, 'rust:err');

    // Monitor exit
    _proc.exited.then((code) => {
        const reason = code !== null
            ? `exited with code ${code}`
            : `killed by signal ${_proc?.signalCode || 'unknown'}`;
        console.log(`[rust-proc] Rust proxy ${reason}`);

        if (!_cleaningUp && _restartCount < MAX_RESTARTS) {
            _restartCount++;
            console.log(`[rust-proc] Auto-restarting in ${RESTART_DELAY_MS / 1000}s (attempt ${_restartCount}/${MAX_RESTARTS})`);
            setTimeout(doStart, RESTART_DELAY_MS);
        } else if (!_cleaningUp) {
            console.log(`[rust-proc] Max restarts (${MAX_RESTARTS}) reached, giving up`);
        }
    });
}

async function pipeStream(
    stream: ReadableStream<Uint8Array> | null,
    prefix: string,
): Promise<void> {
    if (!stream) return;
    const reader = stream.getReader();
    const decoder = new TextDecoder();
    let buf = '';
    try {
        while (true) {
            const { done, value } = await reader.read();
            if (done) break;
            buf += decoder.decode(value, { stream: true });
            const lines = buf.split('\n');
            buf = lines.pop() ?? '';
            for (const line of lines) {
                if (line.trim()) console.log(`[${prefix}] ${line}`);
            }
        }
        if (buf.trim()) console.log(`[${prefix}] ${buf}`);
    } catch {
        // stream closed
    }
}

function startHealthCheckLoop(): void {
    setInterval(async () => {
        try {
            const resp = await fetch(RUST_HEALTH_URL, { signal: AbortSignal.timeout(3000) });
            _lastHealth = { ok: resp.ok, at: Date.now() };
        } catch (err: any) {
            _lastHealth = { ok: false, at: Date.now(), error: err?.message ?? String(err) };
        }
    }, HEALTH_CHECK_INTERVAL_MS);
}
