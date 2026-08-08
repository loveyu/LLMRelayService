/**
 * 渠道管理里那些「不只是读写配置」的操作：连通性测试、拉上游模型、模型元数据覆盖。
 *
 * 控制台（cookie 鉴权的 /__console/api）和对外 OpenAPI（Bearer 鉴权的 /api/v1）
 * 都要用这几件事，逻辑本身跟鉴权方式无关，所以抽在这里，两边路由只负责鉴权和
 * 响应包装。函数统一返回 { status, body }，由调用方决定怎么塞进各自的响应外壳。
 */
import { ensureProviderConfigsLoaded, getChannelModels, getProviderConfig, type ModelInfo } from './config';
import { ensureModelCatalogLoaded, lookupModelContext } from './model-catalog';
import { ensurePricingLoaded, getModelPricing } from './pricing';
import { getModelOverrideKey, listModelMetadataOverrides, upsertModelMetadataOverride } from './model-metadata-overrides';
import { fetchUpstreamModelIds } from './upstream-models';

export type AdminStatus = 200 | 400 | 404 | 502;

export interface AdminResult<T = unknown> {
  status: AdminStatus;
  body: T;
}

export interface UpstreamModelsPreviewInput {
  targetBaseUrl?: string;
  type?: 'openai' | 'anthropic';
  authHeader?: string;
  authValue?: string;
}

const PROVIDER_TEST_TIMEOUT_MS = 30000;

function enrichModel(
  model: ModelInfo,
  overrides: Map<string, { context?: number; pricing?: unknown; updatedAt: number }>,
) {
  const override = overrides.get(getModelOverrideKey(model.channelName, model.id));
  const pricing = override?.pricing ?? getModelPricing(model.id);
  const context = override?.context ?? model.context ?? lookupModelContext(model.id);

  return {
    ...model,
    context,
    ...(pricing ? { pricing } : {}),
    ...(override
      ? {
          override: {
            ...(override.context != null ? { context: override.context } : {}),
            ...(override.pricing ? { pricing: override.pricing } : {}),
            updatedAt: override.updatedAt,
          },
        }
      : {}),
  };
}

/** 所有启用渠道的模型，按 openai / anthropic 分组，附带价格与上下文（含手动覆盖）。 */
export async function listChannelModelsWithMetadata(): Promise<{ openai: unknown[]; anthropic: unknown[] }> {
  await ensureProviderConfigsLoaded();
  await Promise.all([ensureModelCatalogLoaded(), ensurePricingLoaded()]);

  const rawModels = getChannelModels();
  const overrides = await listModelMetadataOverrides();

  return {
    openai: rawModels.filter((m) => m.type === 'openai').map((m) => enrichModel(m, overrides)),
    anthropic: rawModels.filter((m) => m.type === 'anthropic').map((m) => enrichModel(m, overrides)),
  };
}

/** 手动覆盖某个渠道模型的上下文长度和价格。 */
export async function setChannelModelMetadata(
  channelName: string,
  modelId: string,
  payload: unknown,
): Promise<AdminResult> {
  await ensureProviderConfigsLoaded();

  const model = getChannelModels().find((item) => item.channelName === channelName && item.id === modelId);
  if (!model) {
    return { status: 404, body: { error: '模型不存在' } };
  }

  try {
    const override = await upsertModelMetadataOverride(channelName, modelId, payload as any);
    await Promise.all([ensureModelCatalogLoaded(), ensurePricingLoaded()]);
    const pricing = override?.pricing ?? getModelPricing(model.id);
    const context = override?.context ?? model.context ?? lookupModelContext(model.id);

    return {
      status: 200,
      body: {
        ...model,
        context,
        ...(pricing ? { pricing } : {}),
        ...(override
          ? {
              override: {
                ...(override.context != null ? { context: override.context } : {}),
                ...(override.pricing ? { pricing: override.pricing } : {}),
                updatedAt: override.updatedAt,
              },
            }
          : {}),
      },
    };
  } catch (error) {
    return { status: 400, body: { error: error instanceof Error ? error.message : String(error) } };
  }
}

/** 用已保存的渠道配置去问上游 /models。 */
export async function listUpstreamModelsForChannel(channelName: string): Promise<AdminResult> {
  await ensureProviderConfigsLoaded();

  const provider = getProviderConfig(channelName);
  if (!provider) {
    return { status: 404, body: { error: 'Provider 不存在' } };
  }

  const auth = provider.auth;
  if (!auth?.value) {
    return { status: 400, body: { error: '该渠道未配置认证信息，无法请求上游 models 接口' } };
  }

  try {
    const ids = await fetchUpstreamModelIds({
      targetBaseUrl: provider.targetBaseUrl,
      type: provider.type === 'anthropic' ? 'anthropic' : 'openai',
      authHeader: auth.header,
      authValue: auth.value,
    });
    return { status: 200, body: { models: ids.map((id) => ({ id })) } };
  } catch (err) {
    return { status: 502, body: { error: err instanceof Error ? err.message : String(err) } };
  }
}

/** 用请求里现给的连接参数问上游 /models（渠道还没保存时用）。 */
export async function previewUpstreamModels(input: UpstreamModelsPreviewInput): Promise<AdminResult> {
  const baseUrl = (input.targetBaseUrl ?? '').replace(/\/$/, '');
  if (!baseUrl) {
    return { status: 400, body: { error: 'targetBaseUrl 不能为空' } };
  }
  if (!input.authValue) {
    return { status: 400, body: { error: '未填写认证信息（Credential），无法请求上游 models 接口' } };
  }

  const headerName = input.authHeader && input.authHeader !== 'auto'
    ? (input.authHeader as 'x-api-key' | 'authorization')
    : input.type === 'anthropic' ? 'x-api-key' : 'authorization';

  try {
    const ids = await fetchUpstreamModelIds({
      targetBaseUrl: baseUrl,
      type: input.type === 'anthropic' ? 'anthropic' : 'openai',
      authHeader: headerName,
      authValue: input.authValue,
    });
    return { status: 200, body: { models: ids.map((id) => ({ id })) } };
  } catch (err) {
    return { status: 502, body: { error: err instanceof Error ? err.message : String(err) } };
  }
}

/**
 * 拿渠道自己的凭据发一条最小请求，验证「地址 + 密钥 + 模型」这条链路通不通。
 * 上游返回错误时仍然是 HTTP 200，结果放在 body.status 里，方便前端逐个渠道展示。
 */
export async function testProviderConnectivity(
  channelName: string,
  requestedModel?: string,
): Promise<AdminResult> {
  // 测试逻辑已迁移到 Rust（POST /admin/providers/:channel/test）。
  // TS 这里只做本机转发：用 GATEWAY_API_KEY 作为管理凭证调用 Rust，由 Rust 用渠道
  // 自己的 targetBaseUrl + 真实凭证直连上游。返回结构与原实现一致（{ status, body }），
  // 上游 HTTP 状态由 Rust 原样返回，调用方（/__console 与 /api/v1）无需改动。
  const rustHost = process.env.RUST_PROXY_HOST || '127.0.0.1';
  const rustPort = parseInt(process.env.RUST_PROXY_PORT || '3301', 10);
  const url = `http://${rustHost}:${rustPort}/admin/providers/${encodeURIComponent(channelName)}/test`;

  const controller = new AbortController();
  // Rust 内部已有 30s 上游超时，这里多给 5s 余量覆盖本机转发往返。
  const timeoutId = setTimeout(() => controller.abort(), PROVIDER_TEST_TIMEOUT_MS + 5000);
  try {
    const resp = await fetch(url, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'x-api-key': process.env.GATEWAY_API_KEY ?? '',
      },
      body: JSON.stringify(requestedModel ? { model: requestedModel } : {}),
      signal: controller.signal,
    });
    const body = await resp.json().catch(() => ({ error: 'Rust 测试端点返回了非 JSON 响应' }));
    return { status: resp.status as AdminStatus, body };
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    return {
      status: 200,
      body: {
        status: 'error',
        statusCode: 0,
        message: `Rust 测试端点不可达: ${message}`,
      },
    };
  } finally {
    clearTimeout(timeoutId);
  }
}
