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
      case 'initial_rate_limit_snapshot':
        await saveInitialRateLimitSnapshot(msg);
        break;
    }
  } catch (err: any) {
    console.warn('[log-writer] Failed to save log:', err?.message ?? err);
  }
}

async function saveInitialRateLimitSnapshot(msg: any) {
  const { saveConsoleInitialRateLimitSnapshot } = await import('./console-store');
  await saveConsoleInitialRateLimitSnapshot({
    request_id: msg.requestId,
    route_prefix: msg.routePrefix,
    target_url: msg.targetUrl,
    request_model: msg.requestModel,
    forwarded_payload: msg.forwardedPayload ?? null,
    forward_headers: msg.forwardHeaders ?? {},
    response_headers: msg.responseHeaders ?? {},
    response_payload: msg.responsePayload ?? null,
    response_payload_truncated: msg.responsePayloadTruncated ?? false,
  });
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
    failover_from: msg.failoverFrom ?? null,
    failover_chain: msg.failoverChain ?? [],
    failover_reason: msg.failoverReason ?? null,
    initial_response_status: msg.initialResponseStatus ?? null,
    initial_response_status_text: msg.initialResponseStatusText ?? null,
    initial_completed_at: msg.initialCompletedAt ?? null,
    original_route_prefix: msg.originalRoutePrefix ?? null,
    original_request_model: msg.originalRequestModel ?? null,
    retry_attempt: msg.retryAttempt ?? 0,
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
    response_payload: msg.responsePayload ?? null,
    response_payload_truncated: false,
    response_timing: {
      response_body_bytes: msg.responseBodyBytes ?? 0,
      first_chunk_at: msg.firstChunkAt ?? null,
      first_token_at: msg.firstTokenAt ?? null,
      completed_at: msg.completedAt ?? null,
      has_streaming_content: msg.hasStreamingContent ?? false,
      disconnect_source: msg.disconnectSource ?? null,
      disconnected_at: msg.disconnectedAt ?? null,
    },
    response_usage: {
      model: msg.responseModel ?? '',
      stop_reason: msg.stopReason ?? '',
      input_tokens: msg.inputTokens ?? 0,
      output_tokens: msg.outputTokens ?? 0,
      total_tokens: msg.totalTokens ?? 0,
      cache_creation_input_tokens: msg.cacheCreationInputTokens ?? 0,
      cache_read_input_tokens: msg.cacheReadInputTokens ?? 0,
      cached_input_tokens: msg.cachedInputTokens ?? 0,
      reasoning_output_tokens: 0,
      ephemeral_5m_input_tokens: 0,
      ephemeral_1h_input_tokens: 0,
    },
  });
  console.log(`[log-writer] Response saved: ${msg.requestId.substring(0,8)} status=${msg.responseStatus}`);
}

connect();
console.log('[log-writer] Started');
