import { randomUUID } from 'crypto';
import { asc, eq, inArray } from 'drizzle-orm';
import { createDbClient } from './db/client';
import { concurrencyRules, consoleProviders } from './db/schema';

const db = createDbClient();

export interface ConcurrencyRule {
  id: string;
  name: string;
  maxConcurrency: number;
  createdAt: number;
  updatedAt: number;
  providerCount: number;
}

function validate(name: unknown, maxConcurrency: unknown): { name: string; maxConcurrency: number } {
  if (typeof name !== 'string' || !name.trim()) throw new Error('规则名称不能为空');
  const normalizedName = name.trim();
  if (normalizedName.length > 80) throw new Error('规则名称不能超过 80 个字符');
  const normalizedMax = Number(maxConcurrency);
  if (!Number.isSafeInteger(normalizedMax) || normalizedMax < 1 || normalizedMax > 100_000) {
    throw new Error('最大并发必须是 1 到 100000 的整数');
  }
  return { name: normalizedName, maxConcurrency: normalizedMax };
}

export async function listConcurrencyRules(): Promise<ConcurrencyRule[]> {
  const [rules, providers] = await Promise.all([
    db.select().from(concurrencyRules).orderBy(asc(concurrencyRules.name)),
    db.select({ concurrencyRuleId: consoleProviders.concurrencyRuleId }).from(consoleProviders),
  ]);
  const counts = new Map<string, number>();
  for (const provider of providers) {
    if (provider.concurrencyRuleId) counts.set(provider.concurrencyRuleId, (counts.get(provider.concurrencyRuleId) ?? 0) + 1);
  }
  return rules.map((rule) => ({
    id: rule.id,
    name: rule.name,
    maxConcurrency: rule.maxConcurrency,
    createdAt: Number(rule.createdAt),
    updatedAt: Number(rule.updatedAt),
    providerCount: counts.get(rule.id) ?? 0,
  }));
}

export async function getConcurrencyRuleIds(): Promise<Set<string>> {
  const rows = await db.select({ id: concurrencyRules.id }).from(concurrencyRules);
  return new Set(rows.map((row) => row.id));
}

export async function createConcurrencyRule(input: { name: unknown; maxConcurrency: unknown }): Promise<ConcurrencyRule> {
  const value = validate(input.name, input.maxConcurrency);
  const now = Date.now();
  const id = randomUUID();
  await db.insert(concurrencyRules).values({ id, ...value, createdAt: now, updatedAt: now });
  return { id, ...value, createdAt: now, updatedAt: now, providerCount: 0 };
}

export async function updateConcurrencyRule(id: string, input: { name: unknown; maxConcurrency: unknown }): Promise<ConcurrencyRule> {
  const value = validate(input.name, input.maxConcurrency);
  const updatedAt = Date.now();
  const rows = await db.update(concurrencyRules).set({ ...value, updatedAt }).where(eq(concurrencyRules.id, id)).returning();
  if (!rows[0]) throw new Error('并发规则不存在');
  const providerCount = (await db.select({ id: consoleProviders.channelName }).from(consoleProviders).where(eq(consoleProviders.concurrencyRuleId, id))).length;
  return { id, ...value, createdAt: Number(rows[0].createdAt), updatedAt, providerCount };
}

export async function deleteConcurrencyRule(id: string): Promise<void> {
  const bindings = await db.select({ channelName: consoleProviders.channelName }).from(consoleProviders).where(eq(consoleProviders.concurrencyRuleId, id));
  if (bindings.length) throw new Error(`规则仍被渠道使用：${bindings.map((row) => row.channelName).join('、')}`);
  const rows = await db.delete(concurrencyRules).where(eq(concurrencyRules.id, id)).returning({ id: concurrencyRules.id });
  if (!rows.length) throw new Error('并发规则不存在');
}

export async function clearMissingConcurrencyRuleBindings(): Promise<void> {
  const ids = Array.from(await getConcurrencyRuleIds());
  if (ids.length === 0) {
    await db.update(consoleProviders).set({ concurrencyRuleId: null });
  } else {
    const bound = await db.select({ channelName: consoleProviders.channelName, concurrencyRuleId: consoleProviders.concurrencyRuleId })
      .from(consoleProviders);
    const stale = bound.filter((row) => row.concurrencyRuleId && !ids.includes(row.concurrencyRuleId)).map((row) => row.channelName);
    if (stale.length) await db.update(consoleProviders).set({ concurrencyRuleId: null }).where(inArray(consoleProviders.channelName, stale));
  }
}
