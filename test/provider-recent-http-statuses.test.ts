import { afterEach, beforeEach, describe, expect, it } from 'bun:test';
import { createDbClient } from '../src/db/client';
import { consoleRequests } from '../src/db/schema';
import { clearConsoleRequests, getProviderRecentHttpStatuses } from '../src/console-store';

const db = createDbClient();

function requestRow(
  requestId: string,
  channelName: string,
  model: string,
  statusCode: number | null,
  createdAt: number,
  durationMs: number | null,
  sourceRequestType = 'unknown',
) {
  return {
    requestId,
    createdAt,
    routePrefix: channelName,
    method: 'POST',
    path: '/v1/messages',
    targetUrl: 'https://example.com/v1/messages',
    requestModel: model,
    responseStatus: statusCode,
    completedAt: durationMs == null ? null : createdAt + durationMs,
    sourceRequestType,
  };
}

beforeEach(async () => {
  await clearConsoleRequests();
});

afterEach(async () => {
  await clearConsoleRequests();
});

describe('getProviderRecentHttpStatuses', () => {
  it('keeps separate channel and channel-model histories with at most three points', async () => {
    await db.insert(consoleRequests).values([
      requestRow('alpha-a-1', 'alpha', 'model-a', 200, 100, 10),
      requestRow('alpha-a-2', 'alpha', 'model-a', 404, 200, 20),
      requestRow('alpha-a-3', 'alpha', 'model-a', 500, 300, 30),
      requestRow('alpha-a-4', 'alpha', 'model-a', 201, 400, 40),
      requestRow('alpha-b-1', 'alpha', 'model-b', 502, 150, null),
      requestRow('alpha-b-2', 'alpha', 'model-b', 204, 350, 50),
      requestRow('alpha-probe', 'alpha', 'model-a', 503, 500, 60, 'connectivity_test'),
      requestRow('alpha-pending', 'alpha', 'model-a', null, 600, null),
      requestRow('beta-a-1', 'beta', 'model-a', 429, 700, 70),
    ]);

    const result = await getProviderRecentHttpStatuses();

    expect(result.channels.get('alpha')).toEqual([
      { statusCode: 201, createdAt: 400, durationMs: 40 },
      { statusCode: 204, createdAt: 350, durationMs: 50 },
      { statusCode: 500, createdAt: 300, durationMs: 30 },
    ]);
    expect(result.models.get('alpha')?.get('model-a')).toEqual([
      { statusCode: 201, createdAt: 400, durationMs: 40 },
      { statusCode: 500, createdAt: 300, durationMs: 30 },
      { statusCode: 404, createdAt: 200, durationMs: 20 },
    ]);
    expect(result.models.get('alpha')?.get('model-b')).toEqual([
      { statusCode: 204, createdAt: 350, durationMs: 50 },
      { statusCode: 502, createdAt: 150, durationMs: null },
    ]);
    expect(result.models.get('beta')?.get('model-a')).toEqual([
      { statusCode: 429, createdAt: 700, durationMs: 70 },
    ]);
  });
});
