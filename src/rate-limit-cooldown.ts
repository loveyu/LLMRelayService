const BASE_COOLDOWN_MS = 30_000;
const MIN_COOLDOWN_MS = 1_000;
export const MAX_RATE_LIMIT_COOLDOWN_MS = 5 * 60_000;

interface CooldownEntry {
  consecutive429s: number;
  cooldownMs: number;
  expiresAt: number;
}

export interface RateLimitCooldownSnapshot {
  channel: string;
  model: string;
  consecutive429s: number;
  cooldownMs: number;
  remainingMs: number;
  expiresAt: number;
}

const entries = new Map<string, CooldownEntry>();

function key(channel: string, model: string): string {
  return `${channel}\u0000${model}`;
}

function parseKey(value: string): { channel: string; model: string } {
  const separator = value.indexOf('\u0000');
  return { channel: value.slice(0, separator), model: value.slice(separator + 1) };
}

function clampDuration(durationMs: number): number {
  return Math.min(MAX_RATE_LIMIT_COOLDOWN_MS, Math.max(MIN_COOLDOWN_MS, Math.ceil(durationMs)));
}

export function parseRetryAfterMs(value: string | null | undefined, now = Date.now()): number | null {
  const normalized = value?.trim();
  if (!normalized) return null;

  if (/^\d+$/.test(normalized)) {
    return clampDuration(Number(normalized) * 1000);
  }

  const retryAt = Date.parse(normalized);
  if (!Number.isFinite(retryAt)) return null;
  return clampDuration(retryAt - now);
}

export function recordRateLimit429(
  channel: string,
  model: string,
  retryAfter: string | null | undefined,
  enabled: boolean,
  now = Date.now(),
  random = Math.random,
): number {
  const entryKey = key(channel, model);
  if (!enabled) {
    entries.delete(entryKey);
    return 0;
  }

  const consecutive429s = (entries.get(entryKey)?.consecutive429s ?? 0) + 1;
  const retryAfterMs = parseRetryAfterMs(retryAfter, now);
  const exponent = Math.min(consecutive429s - 1, 31);
  const exponentialMs = Math.min(MAX_RATE_LIMIT_COOLDOWN_MS, BASE_COOLDOWN_MS * (2 ** exponent));
  const jitteredMs = exponentialMs * (0.8 + Math.min(1, Math.max(0, random())) * 0.4);
  const cooldownMs = retryAfterMs ?? clampDuration(jitteredMs);
  entries.set(entryKey, { consecutive429s, cooldownMs, expiresAt: now + cooldownMs });
  return cooldownMs;
}

export function getRateLimitCooldownRemainingMs(
  channel: string,
  model: string,
  enabled: boolean,
  now = Date.now(),
): number | null {
  const entryKey = key(channel, model);
  if (!enabled) {
    entries.delete(entryKey);
    return null;
  }
  const entry = entries.get(entryKey);
  if (!entry || entry.expiresAt <= now) return null;
  return entry.expiresAt - now;
}

export function clearRateLimitCooldown(channel: string, model: string): void {
  entries.delete(key(channel, model));
}

export function clearRateLimitCooldowns(channel?: string, model?: string): number {
  let cleared = 0;
  for (const entryKey of entries.keys()) {
    const parsed = parseKey(entryKey);
    if ((channel == null || parsed.channel === channel) && (model == null || parsed.model === model)) {
      entries.delete(entryKey);
      cleared += 1;
    }
  }
  return cleared;
}

export function listRateLimitCooldowns(now = Date.now()): RateLimitCooldownSnapshot[] {
  const result: RateLimitCooldownSnapshot[] = [];
  for (const [entryKey, entry] of entries) {
    if (entry.expiresAt <= now) continue;
    result.push({
      ...parseKey(entryKey),
      consecutive429s: entry.consecutive429s,
      cooldownMs: entry.cooldownMs,
      remainingMs: entry.expiresAt - now,
      expiresAt: entry.expiresAt,
    });
  }
  return result.sort((a, b) => a.expiresAt - b.expiresAt);
}

/** Test-only: module state is process-local and must not leak across cases. */
export function resetRateLimitCooldownsForTest(): void {
  if (process.env.NODE_ENV !== 'test') {
    throw new Error('resetRateLimitCooldownsForTest is only available while running tests');
  }
  entries.clear();
}
