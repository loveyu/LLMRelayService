/**
 * OpenAI 兼容 /models 端点的响应构造。
 *
 * 单独成模块的原因:构造逻辑必须是纯函数(只依赖传入的模型列表),
 * 便于单元测试,也避免测试 import index.ts 时触发 Hono 应用初始化。
 */

import type { ModelInfo } from './config';
import { lookupModelContext } from './model-catalog';

/**
 * codex-cli(>=0.148) 的 ModelInfo 最小合法结构。
 * 字段名与取值参考 codex 官方 models 响应
 * (codex-rs/protocol/src/openai_models.rs 的 ModelInfo serde 定义)。
 * codex 侧必填字段一个都不能少,否则解码失败。
 */
interface CodexModelInfo {
  slug: string;
  display_name: string;
  description: string | null;
  default_reasoning_level: string;
  supported_reasoning_levels: Array<{ effort: string; description: string }>;
  shell_type: string;
  visibility: string;
  supported_in_api: boolean;
  priority: number;
  availability_nux: null;
  upgrade: null;
  support_verbosity: boolean;
  default_verbosity: string;
  apply_patch_tool_type: string;
  truncation_policy: { mode: string; limit: number };
  experimental_supported_tools: string[];
  context_window?: number;
}

/**
 * 为每个模型构造 codex-cli 可解码的 ModelInfo。
 *
 * codex-cli 0.148 起在启动时请求 {base_url}/models 并按自有结构
 * (顶层 `models` 数组)解码;只返回 OpenAI 标准的
 * `{"object":"list","data":[...]}` 会因缺少 `models` 字段解码失败,
 * TUI 无限重试、状态栏停在 "model: loading"。
 *
 * 兼容性:codex 的 ModelsResponse 不拒绝未知字段(多出的 object/data
 * 不影响它),标准 OpenAI 客户端同样忽略未知顶层字段,双方互不干扰。
 */
function buildCodexModelInfos(models: ModelInfo[]): CodexModelInfo[] {
  return models.map((model, index) => {
    const contextWindow = model.context ?? lookupModelContext(model.id);
    return {
      slug: model.id,
      display_name: model.id,
      description: null,
      // 通用档位:覆盖 codex 默认可选项,不假设上游模型支持 xhigh 及以上
      default_reasoning_level: 'medium',
      supported_reasoning_levels: [
        { effort: 'low', description: 'Fast responses with lighter reasoning' },
        { effort: 'medium', description: 'Balances speed and reasoning depth for everyday tasks' },
        { effort: 'high', description: 'Greater reasoning depth for complex problems' },
      ],
      shell_type: 'shell_command',
      visibility: 'list',
      supported_in_api: true,
      priority: index + 1,
      availability_nux: null,
      upgrade: null,
      support_verbosity: true,
      default_verbosity: 'medium',
      apply_patch_tool_type: 'freeform',
      truncation_policy: { mode: 'tokens', limit: 10000 },
      experimental_supported_tools: [],
      ...(contextWindow !== undefined ? { context_window: contextWindow } : {}),
    };
  });
}

/**
 * 构造 /v1/models 与 /openai/v1/models 的响应体。
 * 调用方负责按 UpstreamType 过滤模型列表。
 */
export function buildOpenAiModelsPayload(models: ModelInfo[]): {
  object: string;
  data: Array<Record<string, unknown>>;
  models: CodexModelInfo[];
} {
  return {
    object: 'list',
    data: models.map((model) => {
      const contextWindow = model.context ?? lookupModelContext(model.id);
      return {
        id: model.id,
        object: 'model',
        created: 0,
        owned_by: 'ai-proxy',
        ...(contextWindow !== undefined ? { context_window: contextWindow } : {}),
      };
    }),
    models: buildCodexModelInfos(models),
  };
}
