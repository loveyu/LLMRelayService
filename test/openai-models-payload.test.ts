/**
 * buildOpenAiModelsPayload 单元测试。
 *
 * 重点验证两件事:
 * 1. OpenAI 标准结构(object/data)保持不变,既有客户端不受影响;
 * 2. 新增的顶层 models 数组满足 codex-cli(>=0.148) ModelInfo 的
 *    全部必填字段,否则 codex 解码失败会在启动时无限重试。
 */

import { describe, expect, test } from 'bun:test';
import { buildOpenAiModelsPayload } from '../src/openai-models-payload';
import type { ModelInfo } from '../src/config';

const sampleModels: ModelInfo[] = [
  { id: 'gpt-5.6-sol', channelName: 'ch-a', type: 'openai', context: 272000 },
  { id: 'glm-5.2', channelName: 'ch-b', type: 'openai' },
];

describe('buildOpenAiModelsPayload', () => {
  test('保留 OpenAI 标准 object/data 结构', () => {
    const payload = buildOpenAiModelsPayload(sampleModels);
    expect(payload.object).toBe('list');
    expect(payload.data).toHaveLength(2);
    expect(payload.data[0]).toEqual({
      id: 'gpt-5.6-sol',
      object: 'model',
      created: 0,
      owned_by: 'ai-proxy',
      context_window: 272000,
    });
    // catalog 未加载时 lookupModelContext 返回 undefined,不应输出 context_window
    expect(payload.data[1]).not.toHaveProperty('context_window');
  });

  test('models 数组包含 codex-cli 必填字段', () => {
    const payload = buildOpenAiModelsPayload(sampleModels);
    expect(payload.models).toHaveLength(2);
    const first = payload.models[0]!;
    // codex-rs/protocol/src/openai_models.rs ModelInfo 无 #[serde(default)] 的字段
    for (const key of [
      'slug',
      'display_name',
      'supported_reasoning_levels',
      'shell_type',
      'visibility',
      'supported_in_api',
      'priority',
      'availability_nux',
      'upgrade',
      'support_verbosity',
      'default_verbosity',
      'apply_patch_tool_type',
      'truncation_policy',
      'experimental_supported_tools',
    ]) {
      expect(key in first).toBe(true);
    }
    expect(first.slug).toBe('gpt-5.6-sol');
    expect(first.description).toBeNull();
    expect(Array.isArray(first.supported_reasoning_levels)).toBe(true);
    expect(first.truncation_policy).toEqual({ mode: 'tokens', limit: 10000 });
    // context 优先取模型自身配置
    expect(first.context_window).toBe(272000);
    expect(payload.models[1]).not.toHaveProperty('context_window');
    // priority 用于 codex TUI /model 列表排序,应互不相同
    expect(payload.models[1]!.priority).toBe(2);
  });

  test('空模型列表时 models 为空数组(codex 解码仍合法)', () => {
    const payload = buildOpenAiModelsPayload([]);
    expect(payload.data).toEqual([]);
    expect(payload.models).toEqual([]);
  });
});
