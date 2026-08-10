/**
 * Rust proxy process manager. TS manages the Rust child process lifecycle:
 *  - Spawn on startup
 *  - Auto-restart on crash (max 5 times, 3s delay)
 *  - Health check via HTTP /health
 *  - Expose status for console UI
 */

import { spawn, type Subprocess } from 'bun';
import { isBridgeConnected } from './rust-bridge';

const RUST_HOST = process.env.RUST_PROXY_HOST || '127.0.0.1';
const RUST_PORT = parseInt(process.env.RUST_PROXY_PORT || '3301', 10);
const RUST_HEALTH_URL = `http://${RUST_HOST}:${RUST_PORT}/health`;
const MAX_RESTARTS = 5;
const RESTART_DELAY_MS = 3000;
const HEALTH_CHECK_INTERVAL_MS = 15000;
const IPC_SOCKET = process.env.LRS_IPC_SOCKET || '/tmp/lrs-ipc.sock';

let _proc: Subprocess | null = null;
let _startedAt: number = 0;
let _restartCount: number = 0;
let _lastHealth: { ok: boolean; at: number; error?: string } = { ok: false, at: 0 };
let _cleaningUp = false;
// 每次 doStart 递增。用于区分「当前进程」与「已被 restart 替换的旧进程」：
// 旧进程的 exited 回调看到 generation 已变就不再 auto-restart，避免与
// restartRustProxy 的 doStart 并发 spawn 多个 rust-proxy 抢端口/IPC socket。
let _generation = 0;

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
    /** IPC Unix socket 路径（rust-proxy ←→ TS bridge 通信通道）。 */
    ipcSocket: string;
    /** TS bridge 是否已连上 rust-proxy 的 IPC socket（未连上则配置/日志无法同步）。 */
    bridgeConnected: boolean;
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
        ipcSocket: IPC_SOCKET,
        bridgeConnected: isBridgeConnected(),
    };
}

/** Start the Rust proxy (call once on TS startup). */
export function startRustProxy(): void {
    if (_cleaningUp) return;
    doStart();
    startHealthCheckLoop();
}

// restartRustProxy 并发锁：UI「重启代理」与 bridge auto-restart 可能同时触发,
// 这里做单飞(single-flight)——进行中的重启期间, 后到的请求直接复用同一个 Promise,
// 避免 kill/doStart 并发 spawn 多个 rust-proxy 抢 3311 与 IPC socket。
let _restartInFlight: Promise<{ ok: boolean; error?: string }> | null = null;

/** Kill and restart the Rust proxy. 并发安全（进行中的重启会单飞合并）。 */
export function restartRustProxy(): Promise<{ ok: boolean; error?: string }> {
    if (_cleaningUp) return Promise.resolve({ ok: false, error: 'Shutting down' });
    if (_restartInFlight) {
        console.log('[rust-proc] restart 已在进行, 合并本次并发请求');
        return _restartInFlight;
    }
    _restartInFlight = doRestart().finally(() => { _restartInFlight = null; });
    return _restartInFlight;
}

async function doRestart(): Promise<{ ok: boolean; error?: string }> {
    const oldProc = _proc;
    // 先 bump generation，使下面 kill 触发的 oldProc.exited 回调判定为「已被替换」，
    // 不再走 auto-restart 分支（否则会与本函数末尾的 doStart 并发 spawn 第二个进程）。
    _generation++;
    if (oldProc && oldProc.exitCode === null) {
        oldProc.kill('SIGTERM');
        // Wait up to 5s for graceful shutdown
        for (let i = 0; i < 50; i++) {
            if (oldProc.exitCode !== null) break;
            await new Promise(r => setTimeout(r, 100));
        }
        if (oldProc.exitCode === null) {
            oldProc.kill('SIGKILL');
        }
    }

    // 手动重启不算崩溃，重置计数，给新进程留满 auto-restart 额度。
    _restartCount = 0;
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
    const myGen = ++_generation;
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
    pipeStream(_proc.stdout, 'rust', console.log);
    pipeStream(_proc.stderr, 'rust:err', console.error);

    // Monitor exit
    const proc = _proc;
    proc.exited.then((code) => {
        const reason = code !== null
            ? `exited with code ${code}`
            : `killed by signal ${proc.signalCode || 'unknown'}`;
        console.log(`[rust-proc] Rust proxy ${reason}`);

        // 仅当这是「当前最新一代」的进程意外退出时才 auto-restart。
        // 若 generation 已变（被 restartRustProxy 主动替换），说明新进程已由
        // restartRustProxy 的 doStart 启动，这里不能再 spawn，否则两个 rust-proxy
        // 抢 3311 端口与 IPC socket，最终 socket 路径指向已死进程、bridge 连不上。
        if (_generation !== myGen || _cleaningUp) return;

        if (_restartCount < MAX_RESTARTS) {
            _restartCount++;
            console.log(`[rust-proc] Auto-restarting in ${RESTART_DELAY_MS / 1000}s (attempt ${_restartCount}/${MAX_RESTARTS})`);
            setTimeout(doStart, RESTART_DELAY_MS);
        } else {
            console.log(`[rust-proc] Max restarts (${MAX_RESTARTS}) reached, giving up`);
        }
    });
}

async function pipeStream(
    stream: ReadableStream<Uint8Array> | number | null | undefined,
    prefix: string,
    write: (...args: unknown[]) => void,
): Promise<void> {
    if (!stream || typeof stream === 'number') return;
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
                if (line.trim()) write(`[${prefix}] ${line}`);
            }
        }
        if (buf.trim()) write(`[${prefix}] ${buf}`);
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
