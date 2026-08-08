/**
 * standalone rust-log-writer.ts
 * Connects to Rust IPC socket, receives log messages, writes to DB.
 * Run independently alongside the main TS server.
 */
import { createConnection } from 'node:net';
import { createDbClient } from './db/client';

const IPC_SOCKET = process.env.LRS_IPC_SOCKET || '/tmp/lrs-ipc.sock';
const RECONNECT_MS = 1000;

const db = createDbClient();

let socket: ReturnType<typeof createConnection> | null = null;
let buf = Buffer.alloc(0);

function connect() {
  if (socket) return;
  console.log('[log-writer] Connecting to Rust IPC...');
  socket = createConnection(IPC_SOCKET);

  socket.on('connect', () => {
    console.log('[log-writer] Connected');
    buf = Buffer.alloc(0);
  });

  socket.on('data', (chunk: Buffer) => {
    buf = Buffer.concat([buf, chunk]);
    while (buf.length >= 4) {
      const len = buf.readUInt32BE(0);
      if (len > 16 * 1024 * 1024 || buf.length < 4 + len) break;
      const payload = buf.subarray(4, 4 + len);
      buf = buf.subarray(4 + len);
      handleMessage(payload);
    }
  });

  socket.on('close', () => {
    console.log('[log-writer] Disconnected, reconnecting...');
    socket = null;
    setTimeout(connect, RECONNECT_MS);
  });

  socket.on('error', (err: any) => {
    if (err.code === 'ENOENT' || err.code === 'ECONNREFUSED') {
      console.log('[log-writer] Rust not ready, retrying...');
    } else {
      console.error('[log-writer] Error:', err.message);
    }
    if (socket) { socket.destroy(); socket = null; }
    setTimeout(connect, RECONNECT_MS);
  });
}

async function handleMessage(frame: Buffer) {
  let msg: any;
  try {
    msg = JSON.parse(frame.toString());
  } catch {
    return;
  }

  try {
    switch (msg.type) {
      case 'request_log':
        await saveRequestLog(msg);
        break;
      case 'response_log':
        await saveResponseLog(msg);
        break;
    }
  } catch (err: any) {
    console.warn('[log-writer] Failed to save log:', err?.message ?? err);
  }
}

async function saveRequestLog(msg: any) {
  const { saveConsoleRequest } = await import('./console-store');
  await saveConsoleRequest({
    request_id: msg.requestId,
    created_at: msg.createdAt,
    route_prefix: msg.routePrefix,
    upstream_type: msg.upstreamType,
    method: msg.method,
    path: msg.path,
    target_url: msg.targetUrl,
    request_model: msg.requestModel,
    original_payload: msg.originalPayload ?? null,
    original_payload_truncated: false,
    original_summary: null,
    forwarded_payload: msg.forwardedPayload ?? null,
    forwarded_payload_truncated: false,
    forwarded_summary: null,
    original_headers: msg.originalHeaders ?? {},
    forward_headers: msg.forwardHeaders ?? {},
    api_key_id: msg.apiKeyId ?? null,
    api_key_name: msg.apiKeyName ?? null,
    failover_from: null,
    failover_chain: [],
    failover_reason: null,
    original_route_prefix: null,
    original_request_model: null,
    retry_attempt: 0,
    source_request_type: msg.sourceRequestType ?? 'chat_completion',
  } as any);
  console.log(`[log-writer] Request saved: ${msg.requestId.substring(0,8)} model=${msg.requestModel}`);
}

async function saveResponseLog(msg: any) {
  const { saveConsoleResponse } = await import('./console-store');
  await saveConsoleResponse({
    request_id: msg.requestId,
    response_status: msg.responseStatus,
    response_status_text: msg.responseStatusText,
    response_headers: msg.responseHeaders,
    response_body_bytes: msg.responseBodyBytes,
    first_chunk_at: msg.firstChunkAt ?? undefined,
    first_token_at: msg.firstTokenAt ?? undefined,
    completed_at: msg.completedAt ?? undefined,
    has_streaming_content: msg.hasStreamingContent,
    response_model: msg.responseModel ?? undefined,
    stop_reason: msg.stopReason ?? undefined,
    response_usage: {
      model: msg.responseModel ?? '',
      input_tokens: msg.inputTokens ?? 0,
      output_tokens: msg.outputTokens ?? 0,
      total_tokens: msg.totalTokens ?? 0,
    },
  } as any);
  console.log(`[log-writer] Response saved: ${msg.requestId.substring(0,8)} status=${msg.responseStatus}`);
}

connect();
console.log('[log-writer] Started');
