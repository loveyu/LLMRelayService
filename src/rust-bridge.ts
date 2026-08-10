/**
 * Rust Bridge — IPC client for communicating with the Rust proxy process.
 *
 * Responsibilities:
 *  - Connect to Rust's Unix Domain Socket IPC server
 *  - Send full configuration snapshots on startup and after admin mutations
 *  - Receive request/response logs from Rust and persist to DB
 *  - Handle config reload requests from Rust
 */

import { createConnection } from 'node:net';

const IPC_SOCKET_PATH = process.env.LRS_IPC_SOCKET || '/tmp/lrs-ipc.sock';
const RECONNECT_DELAY_MS = 1000;
const MAX_FRAME_LENGTH = 16 * 1024 * 1024;

// ── Types matching Rust enums ──────────────────────────────────────────────

type TsToRustMessage =
  | { type: 'sync_config'; payload: SyncConfigPayload }
  | { type: 'reload_config' }
  | { type: 'ping'; timestamp: number };

type RustToTsMessage =
  | { type: 'request_log' } & RustRequestLog
  | { type: 'response_log' } & RustResponseLog
  | { type: 'request_config_sync' }
  | { type: 'pong'; timestamp: number };

interface SyncConfigPayload {
  providers: Record<string, ConfigEntry>;
  aliases: Record<string, AliasTarget>;
  failover: GatewayFailoverPolicy;
  timeouts: GatewayTimeoutSettings;
  api_keys: ApiKeyInfo[];
}

interface ConfigEntry {
  type?: 'openai' | 'anthropic';
  targetBaseUrl: string;
  systemPrompt?: string;
  auth?: { header: 'x-api-key' | 'authorization'; value: string };
  models?: { model: string; context?: number; [key: string]: unknown }[];
  priority?: number;
  enabled?: boolean;
  routingVisibility?: 'direct' | 'explicit_only';
  responsesMode?: 'native' | 'chat_compat' | 'disabled';
  extraFields?: Record<string, unknown>;
  providerUuid?: string;
  autoSyncModels?: boolean;
  claudeCodeCompat?: boolean;
}

interface AliasTarget {
  provider: string;
  model: string;
  targets?: { provider: string; model: string }[];
  visible?: boolean;
  returnRealModel?: boolean;
}

interface GatewayFailoverPolicy {
  enabled: boolean;
  retryAttempts: number;
  modelFallbackMode: 'disabled' | 'same_model' | 'any_model';
  maxFallbackAttempts: number;
  customModelFallbacks: { model: string; fallbacks: string[] }[];
  retryOnTimeout: boolean;
  retryOnNetworkError: boolean;
  retryOnStatusCodes: number[];
  retryOnStatusRanges: ('5xx')[];
}

interface GatewayTimeoutSettings {
  defaultFirstByteTimeoutMs: number;
  streamFirstByteTimeoutMs: number;
  imageFirstByteTimeoutMs: number;
  responseIdleTimeoutMs: number;
}

interface ApiKeyInfo {
  id: string;
  name: string;
  keyHash: string;
  allowedModels: string[];
  costQuota: number | null;
  costUsed: number;
  quotaExhausted: boolean;
}

interface RustRequestLog {
  requestId: string;
  createdAt: number;
  method: string;
  routePrefix: string;
  upstreamType: string;
  path: string;
  url: string;
  targetUrl: string;
  requestModel: string;
  originalPayload: string | null;
  forwardedPayload: string | null;
  originalHeaders: Record<string, string>;
  forwardHeaders: Record<string, string>;
  apiKeyId: string | null;
  apiKeyName: string | null;
  /** 请求来源类型：真实转发 "chat_completion"，连通性测试 "connectivity_test"。 */
  sourceRequestType?: string;
  /** 故障转移信息（由 rust-proxy 填充，与 TS index.ts 转发路径语义一致）。 */
  failoverFrom?: string | null;
  failoverChain?: string[];
  failoverReason?: string | null;
  originalRoutePrefix?: string | null;
  originalRequestModel?: string | null;
  retryAttempt?: number;
}

interface RustResponseLog {
  requestId: string;
  responseStatus: number;
  responseStatusText: string;
  responseHeaders: Record<string, string>;
  responseBodyBytes: number;
  firstChunkAt: number | null;
  firstTokenAt: number | null;
  completedAt: number | null;
  hasStreamingContent: boolean;
  responseModel: string | null;
  stopReason: string | null;
  inputTokens: number | null;
  outputTokens: number | null;
  totalTokens: number | null;
  cacheCreationInputTokens: number | null;
  cacheReadInputTokens: number | null;
  cachedInputTokens: number | null;
  responsePayload: string | null;
}

// ── Frame codec (mirrors Rust FrameCodec) ──────────────────────────────────

function encodeFrame(payload: Uint8Array): Uint8Array {
  const len = payload.byteLength;
  const buf = new Uint8Array(4 + len);
  const view = new DataView(buf.buffer);
  view.setUint32(0, len, false); // big-endian
  buf.set(payload, 4);
  return buf;
}

class FrameDecoder {
  private buf = Buffer.alloc(0);

  push(chunk: Buffer): Buffer[] {
    this.buf = Buffer.concat([this.buf, chunk]);
    const frames: Buffer[] = [];
    while (this.buf.length >= 4) {
      const len = this.buf.readUInt32BE(0);
      if (len > MAX_FRAME_LENGTH) {
        console.warn('[rust-bridge] Frame too large, clearing buffer:', len);
        this.buf = Buffer.alloc(0);
        return frames;
      }
      if (this.buf.length < 4 + len) break;
      frames.push(this.buf.subarray(4, 4 + len));
      this.buf = this.buf.subarray(4 + len);
    }
    return frames;
  }
}

// ── Connection state ───────────────────────────────────────────────────────

let socket: ReturnType<typeof createConnection> | null = null;
let reconnectTimer: ReturnType<typeof setTimeout> | null = null;
let decoder = new FrameDecoder();
/** Resolves when the connection is established and config has been synced */
let connected = false;
let onMessage: ((msg: RustToTsMessage) => void) | null = null;
let lastConnectedAt: number = 0;
// 最近一次「从已连接 → 断开」(下降沿) 的时间。scheduleReconnect 用 Date.now()-lastDisconnectedAt
// 判断「真正持续断开的时长」。原来用 Date.now()-lastConnectedAt（距上次连上的时长），会导致
// bridge 稳定连着跑 30s+ 后，一旦短暂断开（如 UI 点重启代理 kill 了 rust-proxy）就被误判为
// 「断开超阈值」从而触发 auto-restart，与重启端点的 restartRustProxy 并发 → 多进程抢端口循环。
let lastDisconnectedAt: number = 0;
let restartingRust = false;
const DISCONNECT_RESTART_THRESHOLD_MS = 30_000;

// ── Public API ─────────────────────────────────────────────────────────────

export function setMessageHandler(handler: (msg: RustToTsMessage) => void): void {
  onMessage = handler;
}

export function startRustBridge(): void {
  if (socket) return;
  connect();
}

export function stopRustBridge(): void {
  if (reconnectTimer) {
    clearTimeout(reconnectTimer);
    reconnectTimer = null;
  }
  if (socket) {
    socket.destroy();
    socket = null;
  }
  connected = false;
}

export function isBridgeConnected(): boolean {
  return connected;
}

/** Send a full config snapshot to Rust */
export async function syncConfigToRust(): Promise<void> {
  if (!socket || !connected) {
    console.warn('[rust-bridge] Not connected, cannot sync config');
    return;
  }
  const payload = await buildSyncConfigPayload();
  sendMessage({ type: 'sync_config', payload });
}

/** Send any message to Rust */
export function sendMessage(msg: TsToRustMessage): void {
  if (!socket || !connected) {
    console.warn('[rust-bridge] Not connected, dropping message:', msg.type);
    return;
  }
  const json = JSON.stringify(msg);
  const frame = encodeFrame(new TextEncoder().encode(json));
  socket.write(frame);
}

// ── Internal ───────────────────────────────────────────────────────────────

function connect(): void {
  if (socket) return;

  console.log('[rust-bridge] Connecting to IPC socket:', IPC_SOCKET_PATH);
  socket = createConnection(IPC_SOCKET_PATH);

  socket.on('connect', () => {
    console.log('[rust-bridge] Connected to Rust proxy');
    connected = true;
    lastConnectedAt = Date.now();
    restartingRust = false;
    decoder = new FrameDecoder();
    // Send initial config sync
    syncConfigToRust().catch((err) =>
      console.error('[rust-bridge] Initial config sync failed:', err),
    );
  });

  socket.on('data', (chunk: Buffer) => {
    const frames = decoder.push(chunk);
    for (const frame of frames) {
      try {
        const msg = JSON.parse(frame.toString()) as RustToTsMessage;
        if (onMessage) onMessage(msg);
      } catch (err) {
        console.warn('[rust-bridge] Failed to parse message:', err);
      }
    }
  });

  socket.on('close', () => {
    console.log('[rust-bridge] Connection closed');
    if (connected) lastDisconnectedAt = Date.now();
    connected = false;
    socket = null;
    void scheduleReconnect();
  });

  socket.on('error', (err: Error & { code?: string }) => {
    if (err.code === 'ENOENT' || err.code === 'ECONNREFUSED') {
      console.log('[rust-bridge] Rust proxy not ready, retrying...');
    } else {
      console.error('[rust-bridge] Socket error:', err.message);
    }
    if (connected) lastDisconnectedAt = Date.now();
    connected = false;
    if (socket) {
      socket.destroy();
      socket = null;
    }
    void scheduleReconnect();
  });
}

async function scheduleReconnect(): Promise<void> {
  if (reconnectTimer) return;

  // 真正的断开时长：只有持续断开超过阈值（rust-proxy 可能挂了）才 auto-restart。
  // 短暂断开（如 UI 重启代理）disconnectedDuration 接近 0，不会误触发。
  const disconnectedDuration = lastDisconnectedAt ? Date.now() - lastDisconnectedAt : 0;

  // Auto-restart Rust if disconnected for too long
  if (disconnectedDuration > DISCONNECT_RESTART_THRESHOLD_MS && !restartingRust) {
    restartingRust = true;
    console.log(
      `[rust-bridge] Disconnected for ${Math.round(disconnectedDuration / 1000)}s, restarting Rust proxy...`,
    );
    try {
      const { restartRustProxy } = await import('./rust-process');
      const result = await restartRustProxy();
      if (!result.ok) {
        console.error('[rust-bridge] Rust restart failed:', result.error);
        restartingRust = false;
      } else {
        console.log('[rust-bridge] Rust proxy restarted, will reconnect');
      }
    } catch (err) {
      console.error('[rust-bridge] Failed to restart Rust:', err);
      restartingRust = false;
    }
    // Give Rust time to start up and bind the socket
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null;
      connect();
    }, RECONNECT_DELAY_MS * 3);
    return;
  }

  reconnectTimer = setTimeout(() => {
    reconnectTimer = null;
    connect();
  }, RECONNECT_DELAY_MS);
}

async function buildSyncConfigPayload(): Promise<SyncConfigPayload> {
  // Dynamic imports to avoid circular deps at module load time
  const [
    { listConsoleProviderEntries },
    { listModelAliases },
    { getGatewayFailoverPolicy },
    { getGatewayTimeoutSettings },
    { createHash },
  ] = await Promise.all([
    import('./console-provider-store'),
    import('./console-model-alias-store'),
    import('./gateway-failover'),
    import('./gateway-timeouts'),
    import('node:crypto'),
  ]);

  const [providers, aliasesRaw, failoverView, timeoutsView] = await Promise.all([
    listConsoleProviderEntries(),
    listModelAliases(),
    getGatewayFailoverPolicy(),
    getGatewayTimeoutSettings(),
  ]);

  // Build aliases map (only enabled)
  const aliases: Record<string, AliasTarget> = {};
  for (const entry of aliasesRaw) {
    if (entry.enabled) {
      aliases[entry.alias] = {
        provider: entry.provider,
        model: entry.model,
        targets: entry.targets,
        visible: entry.visible,
        returnRealModel: entry.returnRealModel,
      };
    }
  }

  // Build API key list (all non-revoked keys with hashes)
  const { createDbClient } = await import('./db/client');
  const { consoleApiKeys } = await import('./db/schema');
  const { eq } = await import('drizzle-orm');

  const db = createDbClient();
  let apiKeysRaw: any[] = [];
  try {
    apiKeysRaw = await db.select().from(consoleApiKeys)
      .where(eq(consoleApiKeys.revoked, 0))
      .orderBy(consoleApiKeys.createdAt);
  } catch {
    console.warn('[rust-bridge] Failed to load API keys, skipping');
  }

  const api_keys: ApiKeyInfo[] = apiKeysRaw.map((row) => ({
    id: row.id,
    name: row.name,
    keyHash: row.keyHash,
    allowedModels: (() => {
      try { return JSON.parse(row.allowedModelsJson || '[]'); }
      catch { return []; }
    })(),
    costQuota: row.costQuotaMicrousd ?? null,
    costUsed: row.costUsedMicrousd ?? 0,
    quotaExhausted: !!(row.costQuotaMicrousd && row.costUsedMicrousd >= row.costQuotaMicrousd),
  }));

  return {
    providers,
    aliases,
    failover: {
      enabled: failoverView.enabled,
      retryAttempts: failoverView.retryAttempts,
      modelFallbackMode: failoverView.modelFallbackMode,
      maxFallbackAttempts: failoverView.maxFallbackAttempts,
      customModelFallbacks: failoverView.customModelFallbacks,
      retryOnTimeout: failoverView.retryOnTimeout,
      retryOnNetworkError: failoverView.retryOnNetworkError,
      retryOnStatusCodes: failoverView.retryOnStatusCodes,
      retryOnStatusRanges: failoverView.retryOnStatusRanges,
    },
    timeouts: {
      defaultFirstByteTimeoutMs: timeoutsView.defaultFirstByteTimeoutMs,
      streamFirstByteTimeoutMs: timeoutsView.streamFirstByteTimeoutMs,
      imageFirstByteTimeoutMs: timeoutsView.imageFirstByteTimeoutMs,
      responseIdleTimeoutMs: timeoutsView.responseIdleTimeoutMs,
    },
    api_keys,
  };
}
