import { beforeEach, describe, expect, it } from 'bun:test';
import {
  MAX_RATE_LIMIT_COOLDOWN_MS,
  clearRateLimitCooldown,
  getRateLimitCooldownRemainingMs,
  listRateLimitCooldowns,
  parseRetryAfterMs,
  recordRateLimit429,
  resetRateLimitCooldownsForTest,
} from '../src/rate-limit-cooldown';

describe('rate limit cooldown', () => {
  beforeEach(() => resetRateLimitCooldownsForTest());

  it('caps Retry-After seconds and HTTP dates at five minutes', () => {
    const now = Date.parse('2026-09-15T00:00:00Z');
    expect(parseRetryAfterMs('3600', now)).toBe(MAX_RATE_LIMIT_COOLDOWN_MS);
    expect(parseRetryAfterMs('Tue, 15 Sep 2026 00:10:00 GMT', now)).toBe(MAX_RATE_LIMIT_COOLDOWN_MS);
  });

  it('uses 30, 60, 120, 240, 300 second exponential bases with deterministic jitter', () => {
    const now = Date.parse('2026-09-15T00:00:00Z');
    const durations = Array.from({ length: 6 }, (_, index) => recordRateLimit429(
      'primary',
      'gpt-5',
      null,
      true,
      now + index,
      () => 0.5,
    ));
    expect(durations).toEqual([30_000, 60_000, 120_000, 240_000, 300_000, 300_000]);
  });

  it('isolates entries by channel and model and clears them after success', () => {
    const now = Date.parse('2026-09-15T00:00:00Z');
    recordRateLimit429('primary', 'gpt-5', '30', true, now);
    expect(getRateLimitCooldownRemainingMs('primary', 'gpt-5', true, now)).toBe(30_000);
    expect(getRateLimitCooldownRemainingMs('primary', 'gpt-4', true, now)).toBeNull();
    expect(getRateLimitCooldownRemainingMs('secondary', 'gpt-5', true, now)).toBeNull();

    clearRateLimitCooldown('primary', 'gpt-5');
    expect(listRateLimitCooldowns(now)).toEqual([]);
  });
});
