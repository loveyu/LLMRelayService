/**
 * LLM Gateway - Bun Server 入口
 */

import app from './index';
import { startPerfMonitor } from './perf-monitor';
import { runAutoModelSync } from './config';
import { fetchModelsDevData, MODEL_CATALOG_CACHE_TTL_MS, primeModelCatalogCache, warmModelCatalogFromDb } from './model-catalog';
import { saveCatalogToDb } from './catalog-db';
import { primePricingCache, warmPricingCacheFromDb } from './pricing';
import { initializeTokenEstimator } from './token-estimator';
import { runMigrations, type MigrationStatus } from './db/migrate';
import { getDatabaseUrl, getDbDriver, getSqliteFilePath } from './db/config';
import postgres from 'postgres';
import { createCorsPreflightResponse, withCorsHeaders } from './cors';
import { initLoggerFromEnv, overrideConsole } from './logger';

initLoggerFromEnv();
overrideConsole();

const stubEnv = {
  LLM_STATUS: {
    writeDataPoint: () => {},
  },
};

const PORT = parseInt(process.env.PORT || '3300');
const HOST = process.env.SERVER_HOST || '0.0.0.0';
const IDLE_TIMEOUT_SECONDS = Number.parseInt(process.env.BUN_SERVER_IDLE_TIMEOUT_SECONDS || '0', 10);

// 初始化 token 估算器（WASM tiktoken 一次性初始化）
initializeTokenEstimator();

// 1. 执行数据库迁移（不阻断服务启动，失败时记录状态）
let migrationStatus: MigrationStatus = { state: 'success' };
try {
  migrationStatus = await runMigrations();
} catch (error: any) {
  migrationStatus = { state: 'failed', error: error?.message ?? String(error) };
  console.error('[DB] Migration failed:', error);
}

// 2. 从 DB 预热 catalog 缓存（带保护，数据库不可用时优雅降级）
let dbCatalogFresh = false;
let dbPricingFresh = false;
if (migrationStatus.state === 'success' || migrationStatus.state === 'skipped') {
  try {
    [dbCatalogFresh, dbPricingFresh] = await Promise.all([
      warmModelCatalogFromDb(),
      warmPricingCacheFromDb(),
    ]);
  } catch (error) {
    console.warn('[catalog] Failed to warm from DB:', error);
    dbCatalogFresh = false;
  }
}

async function refreshExternalCatalog(): Promise<void> {
  const result = await fetchModelsDevData();
  if (!result) return;
  const now = Date.now();
  primeModelCatalogCache(result.contextMap, now);
  primePricingCache(result.pricingMap, now);
  await saveCatalogToDb(result.contextMap, result.pricingMap, now);
  console.log(`[catalog] Background refresh: ${result.contextMap.size} context + ${result.pricingMap.size} pricing entries saved`);
}

function escapeHtml(str: string): string {
  return str.replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]!));
}

const degradedHtmlTemplate = await Bun.file(`${import.meta.dir}/degraded.html`).text();

function showMigrationGuide(status: Extract<MigrationStatus, { state: 'failed' }>): Response {
  const html = degradedHtmlTemplate.replace('{{ERROR}}', escapeHtml(status.error));
  return new Response(html, {
    status: 503,
    headers: { 'Content-Type': 'text/html; charset=utf-8' },
  });
}

async function resetDatabaseSqlite(): Promise<{ success: boolean; message?: string; error?: string }> {
  const databaseUrl = getDatabaseUrl();
  try {
    const { Database } = require('bun:sqlite') as typeof import('bun:sqlite');
    const db = new Database(getSqliteFilePath(databaseUrl), { create: true });
    try {
      db.exec('PRAGMA foreign_keys = OFF;');
      const tables = db
        .query("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
        .all() as { name: string }[];
      for (const { name } of tables) {
        db.exec(`DROP TABLE IF EXISTS "${name}"`);
        console.log(`[DB] Dropped table: ${name}`);
      }
    } finally {
      db.close();
    }

    // 重新执行迁移（强制重新执行，不走缓存）
    const result = await runMigrations(undefined, true);
    if (result.state === 'success') {
      return { success: true, message: '数据库已重置并重新迁移' };
    }
    return { success: false, error: result.state === 'failed' ? result.error : '迁移失败' };
  } catch (err: any) {
    return { success: false, error: err?.message ?? String(err) };
  }
}

async function resetDatabase(): Promise<{ success: boolean; message?: string; error?: string }> {
  if (getDbDriver() === 'sqlite') {
    return resetDatabaseSqlite();
  }

  const databaseUrl = getDatabaseUrl();
  const sql = postgres(databaseUrl, { max: 1, prepare: false });

  try {
    // 获取所有用户表（排除 drizzle 系统表）
    const tables = await sql`
      SELECT tablename FROM pg_tables
      WHERE schemaname = 'public'
      AND tablename NOT LIKE 'pg_%'
      AND tablename NOT LIKE 'sql_%'
    `;

    // 删除所有表
    for (const row of tables) {
      const tableName = (row as any).tablename;
      await sql.unsafe(`DROP TABLE IF EXISTS "${tableName}" CASCADE`);
      console.log(`[DB] Dropped table: ${tableName}`);
    }

    // 删除 drizzle schema 和迁移记录
    await sql`DROP SCHEMA IF EXISTS "drizzle" CASCADE`;
    console.log('[DB] Dropped drizzle schema');

    await sql.end();

    // 重新执行迁移（强制重新执行，不走缓存）
    const result = await runMigrations(undefined, true);
    if (result.state === 'success') {
      return { success: true, message: '数据库已重置并重新迁移' };
    }
    return { success: false, error: result.state === 'failed' ? result.error : '迁移失败' };
  } catch (err: any) {
    await sql.end().catch(() => {});
    return { success: false, error: err?.message ?? String(err) };
  }
}

const bunServer = Bun.serve({
  hostname: HOST,
  port: PORT,
  idleTimeout: Number.isFinite(IDLE_TIMEOUT_SECONDS) && IDLE_TIMEOUT_SECONDS >= 0 ? IDLE_TIMEOUT_SECONDS : 0,
  fetch: async (req) => {
    const url = new URL(req.url);

    if (req.method === 'OPTIONS') {
      return createCorsPreflightResponse(req);
    }

    // 健康检查端点
    if (url.pathname === '/health') {
      const isHealthy = migrationStatus.state === 'success' || migrationStatus.state === 'skipped';
      return withCorsHeaders(Response.json({
        status: isHealthy ? 'ok' : 'degraded',
        database: migrationStatus,
      }, { status: isHealthy ? 200 : 503 }), req);
    }

    // 迁移失败时根路径显示指引页
    if (url.pathname === '/' && migrationStatus.state === 'failed') {
      return withCorsHeaders(showMigrationGuide(migrationStatus as Extract<MigrationStatus, { state: 'failed' }>), req);
    }

    // 数据库重置 API（仅在降级模式下可用）
    if (url.pathname === '/api/db/reset' && req.method === 'POST') {
      if (migrationStatus.state !== 'failed') {
        return withCorsHeaders(Response.json({ error: '数据库状态正常，无需重置' }, { status: 400 }), req);
      }
      const result = await resetDatabase();
      if (result.success) {
        // 更新迁移状态
        migrationStatus = { state: 'success' };
        return withCorsHeaders(Response.json({ message: result.message }), req);
      }
      return withCorsHeaders(Response.json({ error: result.error }, { status: 500 }), req);
    }

    return withCorsHeaders(await app.fetch(req, stubEnv as any), req);
  },
});

console.log(`LLM Gateway running on ${HOST}:${PORT} (idleTimeout=${Number.isFinite(IDLE_TIMEOUT_SECONDS) && IDLE_TIMEOUT_SECONDS >= 0 ? IDLE_TIMEOUT_SECONDS : 0}s)`);

if (process.env.PERF_ENABLED !== 'false') startPerfMonitor();

// Start Rust proxy as child process. TS manages its lifecycle:
// auto-restart on crash, health check, status/restart via console API.
import('./rust-process').then(({ startRustProxy: startRust, stopRustProxy }) => {
  startRust();

  // Start IPC bridge immediately — handles ENOENT/ECONNREFUSED with auto-retry
  import('./rust-bridge').then(({ startRustBridge }) => {
    startRustBridge();
  }).catch(() => {});

  // Set up IPC message handler for log forwarding from Rust.
  // Sequential queue — ensures request_log is always inserted before response_log updates.
  let ipcQueue: Promise<void> = Promise.resolve();
  import('./rust-bridge').then(({ setMessageHandler }) => {
    setMessageHandler((msg) => {
      ipcQueue = ipcQueue.then(async () => {
        switch (msg.type) {
          case 'request_log': {
            const { saveConsoleRequest } = await import('./console-store');
            await saveConsoleRequest({
              request_id: msg.requestId,
              created_at: msg.createdAt,
              route_prefix: msg.routePrefix,
              upstream_type: msg.upstreamType as any,
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
              original_route_prefix: msg.originalRoutePrefix ?? null,
              original_request_model: msg.originalRequestModel ?? null,
              retry_attempt: msg.retryAttempt ?? 0,
              source_request_type: msg.sourceRequestType ?? 'chat_completion',
            } as any);
            break;
          }
          case 'response_log': {
            const { saveConsoleResponse } = await import('./console-store');
            await saveConsoleResponse({
              request_id: msg.requestId,
              response_status: msg.responseStatus,
              response_status_text: msg.responseStatusText,
              response_headers: msg.responseHeaders,
              response_payload: msg.responsePayload ?? null,
              response_payload_truncated: false,
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
              response_timing: {
                response_body_bytes: msg.responseBodyBytes ?? 0,
                first_chunk_at: msg.firstChunkAt ?? null,
                first_token_at: msg.firstTokenAt ?? null,
                completed_at: msg.completedAt ?? null,
                has_streaming_content: msg.hasStreamingContent ?? false,
              },
            });
            break;
          }
        }
      }).catch((err: any) => {
        console.warn('[rust-bridge] Failed to save log:', err?.message ?? err);
      });
    });
  }).catch(() => {});

  // Graceful shutdown: stop accepting new requests, drain in-flight, then stop Rust
  let shuttingDown = false;
  const GRACEFUL_TIMEOUT_MS = 15_000;
  const doCleanup = () => {
    if (shuttingDown) return;
    shuttingDown = true;
    console.log('[server] Graceful shutdown initiated...');
    bunServer.stop(); // stop accepting new connections

    // Poll until in-flight requests complete or timeout
    const deadline = Date.now() + GRACEFUL_TIMEOUT_MS;
    const drainAndExit = () => {
      if (Date.now() >= deadline || bunServer.pendingRequests === 0) {
        console.log(`[server] Draining done (${bunServer.pendingRequests} pending), exiting`);
        stopRustProxy().catch(() => {});
        process.exit(0);
      } else {
        setTimeout(drainAndExit, 100);
      }
    };
    drainAndExit();
  };
  process.on('SIGTERM', doCleanup);
  process.on('SIGINT', doCleanup);
}).catch((err) => {
  console.warn('[rust-proc] Failed to start:', err instanceof Error ? err.message : err);
});

// 每 24h 自动同步开启了「自动同步上游模型」的渠道模型列表
const AUTO_MODEL_SYNC_INTERVAL_MS = 24 * 60 * 60 * 1000;
if (migrationStatus.state === 'success' || migrationStatus.state === 'skipped') {
  setInterval(() => {
    runAutoModelSync()
      .then(({ synced, failed }) => {
        if (synced > 0 || failed > 0) {
          console.log(`[auto-sync] 定时任务完成：成功 ${synced} 个，失败 ${failed} 个`);
        }
      })
      .catch((error) => {
        console.warn('[auto-sync] 定时任务异常:', error instanceof Error ? error.message : error);
      });
  }, AUTO_MODEL_SYNC_INTERVAL_MS);
}

// 并行从 models.dev 刷新 catalog（不阻塞启动，DB 缓存过期时才需要）
if (!dbCatalogFresh || !dbPricingFresh) {
  refreshExternalCatalog().catch((error) => {
    console.warn('[catalog] Background refresh failed:', error instanceof Error ? error.message : error);
  });
}

const catalogRefreshTimer = setInterval(() => {
  refreshExternalCatalog().catch((error) => {
    console.warn('[catalog] Scheduled refresh failed:', error instanceof Error ? error.message : error);
  });
}, MODEL_CATALOG_CACHE_TTL_MS);
catalogRefreshTimer.unref();
