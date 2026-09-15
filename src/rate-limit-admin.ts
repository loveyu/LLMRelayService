import {
  MAX_RATE_LIMIT_COOLDOWN_MS,
  clearRateLimitCooldowns,
  listRateLimitCooldowns,
  type RateLimitCooldownSnapshot,
} from './rate-limit-cooldown';

export interface RateLimitCooldownsPayload {
  cooldowns: RateLimitCooldownSnapshot[];
  maxCooldownMs: number;
}

function rustAdminUrl(path: string): string {
  const host = process.env.RUST_PROXY_HOST || '127.0.0.1';
  const port = Number.parseInt(process.env.RUST_PROXY_PORT || '3301', 10);
  return `http://${host}:${port}${path}`;
}

function rustAdminHeaders(): Record<string, string> {
  return {
    Accept: 'application/json',
    'Content-Type': 'application/json',
    'x-api-key': process.env.GATEWAY_API_KEY ?? '',
  };
}

export async function getRateLimitCooldowns(): Promise<RateLimitCooldownsPayload> {
  const localCooldowns = listRateLimitCooldowns();
  try {
    const response = await fetch(rustAdminUrl('/admin/rate-limit-cooldowns'), {
      headers: rustAdminHeaders(),
      signal: AbortSignal.timeout(3_000),
    });
    if (!response.ok) throw new Error(`Rust cooldown endpoint returned ${response.status}`);
    const rust = await response.json() as RateLimitCooldownsPayload;
    const merged = new Map<string, RateLimitCooldownSnapshot>();
    for (const entry of [...localCooldowns, ...rust.cooldowns]) {
      const entryKey = `${entry.channel}\u0000${entry.model}`;
      const current = merged.get(entryKey);
      if (!current || entry.expiresAt > current.expiresAt) merged.set(entryKey, entry);
    }
    return {
      cooldowns: Array.from(merged.values()).sort((a, b) => a.expiresAt - b.expiresAt),
      maxCooldownMs: MAX_RATE_LIMIT_COOLDOWN_MS,
    };
  } catch {
    return { cooldowns: localCooldowns, maxCooldownMs: MAX_RATE_LIMIT_COOLDOWN_MS };
  }
}

export async function clearRateLimitCooldownRuntime(
  channel?: string,
  model?: string,
): Promise<{ cleared: number }> {
  let cleared = clearRateLimitCooldowns(channel, model);
  const response = await fetch(rustAdminUrl('/admin/rate-limit-cooldowns/clear'), {
    method: 'POST',
    headers: rustAdminHeaders(),
    body: JSON.stringify({ channel, model }),
    signal: AbortSignal.timeout(3_000),
  });
  if (!response.ok) {
    throw new Error(`Rust cooldown endpoint returned ${response.status}`);
  }
  const result = await response.json() as { cleared?: number };
  cleared += result.cleared ?? 0;
  return { cleared };
}
