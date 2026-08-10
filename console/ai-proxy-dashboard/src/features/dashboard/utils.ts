import type {
  ConsoleRequestListItem,
  RequestSortKey,
  SortDirection,
} from "@/features/dashboard/types"

export function formatTime(timestamp: number | null | undefined): string {
  if (!timestamp) return "--"
  return new Date(timestamp).toLocaleString("zh-CN", { hour12: false })
}

export function formatBytes(bytes: number | null | undefined): string {
  const value = Number(bytes || 0)
  if (!Number.isFinite(value) || value <= 0) return "0 B"
  if (value < 1024) return `${value} B`
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`
  return `${(value / (1024 * 1024)).toFixed(1)} MB`
}

export function formatCount(value: unknown): string {
  const numeric = Number(value || 0)
  if (!Number.isFinite(numeric)) return "--"
  return numeric.toLocaleString("zh-CN")
}

export function formatDuration(value: unknown): string {
  const numeric = Number(value)
  if (!Number.isFinite(numeric) || numeric < 0) return "--"
  if (numeric < 1000) return `${Math.round(numeric)} ms`
  if (numeric < 60 * 1000) {
    return `${(numeric / 1000).toFixed(numeric >= 10 * 1000 ? 1 : 2)} s`
  }
  return `${(numeric / (60 * 1000)).toFixed(1)} min`
}

export function formatPercent(value: unknown): string {
  const numeric = Number(value)
  if (!Number.isFinite(numeric)) return "--"
  return `${numeric.toFixed(1)}%`
}

export function formatCost(value: unknown): string {
  const numeric = Number(value || 0)
  if (!Number.isFinite(numeric) || numeric < 0) return "--"
  if (numeric === 0) return "$0.00"
  if (numeric < 0.01) return `$${numeric.toFixed(4)}`
  return `$${numeric.toFixed(2)}`
}

/**
 * 成本明细里的金额。列表里的 $0.07 够用，但明细面板要让「单价 × token 数 = 成本」
 * 逐项对得上，两位小数会把 $0.011214 抹成 $0.01、加总看起来对不上，这里统一保留 6 位。
 */
export function formatCostDetailed(value: unknown): string {
  const numeric = Number(value || 0)
  if (!Number.isFinite(numeric) || numeric < 0) return "--"
  return `$${numeric.toFixed(6)}`
}

export function formatPricePerMillion(value: unknown): string {
  const numeric = Number(value || 0)
  if (!Number.isFinite(numeric) || numeric < 0) return "--"
  if (numeric === 0) return "$0.00 / 1M"
  if (numeric < 0.01) return `$${numeric.toFixed(4)} / 1M`
  return `$${numeric.toFixed(2)} / 1M`
}

export function calculateOutputTokensPerSecond(
  usageLike: any,
  timingLike: any,
): number | null {
  const outputTokens = Number(usageLike?.output_tokens ?? usageLike?.total_output_tokens)
  const generationDurationMs = Number(timingLike?.generation_duration_ms)

  if (!Number.isFinite(outputTokens) || outputTokens <= 0) return null
  if (!Number.isFinite(generationDurationMs) || generationDurationMs <= 0) return null

  return outputTokens / (generationDurationMs / 1000)
}

export function formatTokensPerSecond(value: unknown): string {
  const numeric = Number(value)
  if (!Number.isFinite(numeric) || numeric <= 0) return "--"
  if (numeric >= 100) return `${numeric.toFixed(0)} tok/s`
  if (numeric >= 10) return `${numeric.toFixed(1)} tok/s`
  return `${numeric.toFixed(2)} tok/s`
}

export function shortText(text: unknown, max = 18): string {
  const value = String(text ?? "")
  if (!value) return "--"
  if (value.length <= max) return value
  return `${value.slice(0, max)}...`
}

export function calculateHitRate(hits: unknown, requests: unknown): number {
  const hitValue = Number(hits || 0)
  const requestValue = Number(requests || 0)
  if (!requestValue) return 0
  return (hitValue / requestValue) * 100
}

export function getTotalTokens(usageLike: any, upstreamType?: string): number {
  const explicitTotal = Number(usageLike?.total_tokens ?? 0)
  if (Number.isFinite(explicitTotal) && explicitTotal > 0) return explicitTotal

  const input = Number(usageLike?.input_tokens ?? usageLike?.total_input_tokens ?? 0)
  const output = Number(
    usageLike?.output_tokens ?? usageLike?.total_output_tokens ?? 0,
  )
  if (upstreamType === "openai") {
    return input + output
  }
  const cacheCreation = Number(
    usageLike?.cache_creation_input_tokens ??
      usageLike?.total_cache_creation_tokens ??
      0,
  )
  const cacheRead = Number(
    usageLike?.cache_read_input_tokens ?? usageLike?.total_cache_read_tokens ?? 0,
  )

  return input + output + cacheCreation + cacheRead
}

export function getUsageMetricRows(
  usageLike: any,
  timingLike: any,
  upstreamType: string,
): Array<{ label: string; value: string }> {
  const outputSpeed = formatTokensPerSecond(
    calculateOutputTokensPerSecond(usageLike, timingLike),
  )
  const estimatedSuffix = usageLike?.estimated ? " (estimated)" : ""

  if (upstreamType === "openai") {
    return [
      { label: `prompt${estimatedSuffix}`, value: formatCount(usageLike?.input_tokens) },
      {
        label: `uncached prompt${estimatedSuffix}`,
        value: formatCount(
          usageLike?.uncached_input_tokens ?? usageLike?.input_tokens,
        ),
      },
      { label: `completion${estimatedSuffix}`, value: formatCount(usageLike?.output_tokens) },
      { label: `total${estimatedSuffix}`, value: formatCount(getTotalTokens(usageLike, upstreamType)) },
      { label: "输出速度", value: outputSpeed },
      {
        label: "cached prompt",
        value: formatCount(usageLike?.cached_input_tokens),
      },
      {
        label: "reasoning",
        value: formatCount(usageLike?.reasoning_output_tokens),
      },
      { label: "stop reason", value: usageLike?.stop_reason || "--" },
    ]
  }

  return [
    { label: `input${estimatedSuffix}`, value: formatCount(usageLike?.input_tokens) },
    { label: `output${estimatedSuffix}`, value: formatCount(usageLike?.output_tokens) },
    { label: `total${estimatedSuffix}`, value: formatCount(getTotalTokens(usageLike, upstreamType)) },
    { label: "输出速度", value: outputSpeed },
    {
      label: "cache create",
      value: formatCount(usageLike?.cache_creation_input_tokens),
    },
    {
      label: "cache read",
      value: formatCount(usageLike?.cache_read_input_tokens),
    },
    { label: "stop reason", value: usageLike?.stop_reason || "--" },
  ]
}

function formatCostFormula(
  tokens: unknown,
  unitPrice: unknown,
  cost: unknown,
): string {
  return `${formatCount(tokens)} × ${formatPricePerMillion(unitPrice)} = ${formatCostDetailed(cost)}`
}

export function getCostMetricRows(
  usageLike: any,
  fallbackModel?: string,
  upstreamType?: string,
): Array<{ label: string; value: string }> {
  const pricing = usageLike?.cost_pricing
  const breakdown = usageLike?.cost_breakdown
  const resolvedUpstreamType = upstreamType === "openai" ? "openai" : "anthropic"
  const model = usageLike?.model || fallbackModel || "--"
  const uncachedInputTokens =
    breakdown?.uncached_input_tokens ??
    usageLike?.uncached_input_tokens ??
    usageLike?.input_tokens ??
    0
  const cacheReadTokens =
    breakdown?.cache_read_tokens ??
    (resolvedUpstreamType === "openai"
      ? usageLike?.cached_input_tokens
      : usageLike?.cache_read_input_tokens) ??
    0
  const cacheWriteTokens =
    breakdown?.cache_write_tokens ??
    (resolvedUpstreamType === "anthropic"
      ? usageLike?.cache_creation_input_tokens
      : 0) ??
    0
  const cacheWrite1hTokens = breakdown?.cache_write_1h_tokens ?? 0
  const cacheWrite5mTokens = breakdown?.cache_write_5m_tokens ?? cacheWriteTokens - cacheWrite1hTokens
  const pricingModel = usageLike?.cost_pricing_model
  const rows = [
    { label: "总成本", value: formatCostDetailed(usageLike?.cost) },
    { label: "模型", value: model },
  ]

  // 中转上游有时把响应模型写成自己的代号（openclaw / code 之类），这时价格是按请求模型取的，
  // 把实际计价的模型 ID 标出来，否则「模型」一行和单价对不上。
  if (pricingModel && pricingModel !== model) {
    rows.push({ label: "计价模型", value: pricingModel })
  }

  if (!pricing || !breakdown) {
    rows.push({
      label: "计算公式",
      value: `缺少「${model}」的定价数据，可在「模型」页给该渠道模型配置价格后重新计算。`,
    })
    return rows
  }

  // 上游没给缓存价时后端会按 input 单价推导兜底价，这里展示实际计费用的单价，
  // 保证「单价 × token 数 = 成本」在页面上自洽。
  const cacheReadPrice = breakdown.cache_read_price ?? pricing.cache_read ?? 0
  const cacheWrite5mPrice = breakdown.cache_write_5m_price ?? pricing.cache_write ?? 0
  const cacheWrite1hPrice = breakdown.cache_write_1h_price ?? cacheWrite5mPrice
  const derivedSuffix = breakdown.cache_pricing_derived ? "（推导）" : ""

  if (resolvedUpstreamType === "openai") {
    rows.push({
      label: "模型单价",
      value: `输入 ${formatPricePerMillion(pricing.input)} · 输出 ${formatPricePerMillion(pricing.output)} · cached prompt ${formatPricePerMillion(cacheReadPrice)}${derivedSuffix}`,
    })
  } else {
    rows.push({
      label: "模型单价",
      value: `输入 ${formatPricePerMillion(pricing.input)} · 输出 ${formatPricePerMillion(pricing.output)} · 缓存读 ${formatPricePerMillion(cacheReadPrice)} · 缓存写 ${formatPricePerMillion(cacheWrite5mPrice)}${derivedSuffix}`,
    })
  }
  rows.push({
    label: "输入公式",
    value: formatCostFormula(
      uncachedInputTokens,
      pricing.input,
      breakdown.input_cost,
    ),
  })
  rows.push({
    label: "输出公式",
    value: formatCostFormula(
      usageLike?.output_tokens,
      pricing.output,
      breakdown.output_cost,
    ),
  })
  rows.push({
    label: resolvedUpstreamType === "openai" ? "cached prompt公式" : "缓存读公式",
    value: formatCostFormula(
      cacheReadTokens,
      cacheReadPrice,
      breakdown.cache_read_cost,
    ),
  })
  if (resolvedUpstreamType === "anthropic") {
    if (cacheWrite1hTokens > 0) {
      // 1h TTL 缓存写入单价是 5m 的 1.6 倍，分档展示才对得上总成本。
      rows.push({
        label: "缓存写公式 (5m)",
        value: formatCostFormula(
          cacheWrite5mTokens,
          cacheWrite5mPrice,
          (cacheWrite5mTokens / 1_000_000) * cacheWrite5mPrice,
        ),
      })
      rows.push({
        label: "缓存写公式 (1h)",
        value: formatCostFormula(
          cacheWrite1hTokens,
          cacheWrite1hPrice,
          (cacheWrite1hTokens / 1_000_000) * cacheWrite1hPrice,
        ),
      })
    } else {
      rows.push({
        label: "缓存写公式",
        value: formatCostFormula(
          cacheWriteTokens,
          cacheWrite5mPrice,
          breakdown.cache_write_cost,
        ),
      })
    }
  }
  rows.push({
    label: "汇总公式",
    value:
      resolvedUpstreamType === "openai"
        ? `${formatCostDetailed(breakdown.input_cost)} + ${formatCostDetailed(breakdown.output_cost)} + ${formatCostDetailed(breakdown.cache_read_cost)} = ${formatCostDetailed(breakdown.total_cost)}`
        : `${formatCostDetailed(breakdown.input_cost)} + ${formatCostDetailed(breakdown.output_cost)} + ${formatCostDetailed(breakdown.cache_read_cost)} + ${formatCostDetailed(breakdown.cache_write_cost)} = ${formatCostDetailed(breakdown.total_cost)}`,
  })
  return rows
}

export function getStatusBadgeVariant(
  cacheState: string,
): "secondary" | "outline" | "destructive" {
  if (cacheState === "hit") return "secondary"
  if (cacheState === "miss") return "destructive"
  return "outline"
}

export function getComparisonBadgeVariant(
  status: string,
): "secondary" | "outline" | "destructive" {
  if (status === "expected_hit_confirmed") return "secondary"
  if (status === "expected_hit_missed") return "destructive"
  return "outline"
}

export function getHttpStatusLabel(
  status: number | null | undefined,
): string {
  if (status == null) return "请求中"
  return String(status)
}

export function getHttpStatusBadgeVariant(
  status: number | null | undefined,
): "secondary" | "outline" | "destructive" {
  if (status == null) return "outline"
  if (status >= 200 && status < 400) return "secondary"
  return "destructive"
}

export function getPayloadText(payload: string | null | undefined): string {
  const text = String(payload ?? "")
  if (!text) return ""

  try {
    return JSON.stringify(JSON.parse(text), null, 2)
  } catch {
    return text
  }
}

function extractJsonObjectsFromSse(payloadText: string): Array<Record<string, unknown>> {
  const blocks = payloadText
    .split(/\n\n+/)
    .map((block) => block.trim())
    .filter(Boolean)

  const results: Array<Record<string, unknown>> = []

  for (const block of blocks) {
    const dataLines = block
      .split("\n")
      .filter((line) => line.startsWith("data:"))
      .map((line) => line.slice(5).trim())

    if (!dataLines.length) continue

    const rawData = dataLines.join("\n")
    if (!rawData || rawData === "[DONE]") continue

    try {
      const parsed = JSON.parse(rawData) as unknown
      if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
        results.push(parsed as Record<string, unknown>)
      }
    } catch {
      continue
    }
  }

  return results
}

function collectTextFromUnknown(value: unknown): string {
  if (typeof value === "string") return value

  if (Array.isArray(value)) {
    return value.map((item) => collectTextFromUnknown(item)).join("")
  }

  if (!value || typeof value !== "object") return ""

  const record = value as Record<string, unknown>

  if (typeof record.text === "string") return record.text
  if (typeof record.output_text === "string") return record.output_text
  if (typeof record.content === "string") return record.content

  if (Array.isArray(record.content)) {
    return record.content.map((item) => collectTextFromUnknown(item)).join("")
  }

  return ""
}

export type SseReadableSegments = {
  reasoning: string
  content: string
}

// 把流式 SSE 拆成「思考过程」与「内容」两段，避免推理模型的 reasoning_content
// 与正文混在一起难以阅读：
//  - reasoning: OpenAI Chat 的 delta.reasoning_content / reasoning、Anthropic 的
//    thinking 块与 thinking_delta、OpenAI Responses 的 reasoning summary。
//  - content:   其余正文（content / text_delta / tool_use / output_text）。
// Anthropic 流式 tool_use 仍按 index 累积 input_json_delta，在 content_block_stop
// 时输出 [Tool: name] + 美化后的参数 JSON。
export function extractReadableSseSegments(
  payload: string | null | undefined,
): SseReadableSegments {
  const payloadText = String(payload ?? "").trim()
  if (!payloadText) return { reasoning: "", content: "" }

  const jsonObjects = extractJsonObjectsFromSse(payloadText)
  if (!jsonObjects.length) return { reasoning: "", content: "" }

  const reasoningParts: string[] = []
  const contentParts: string[] = []
  const toolUseBuffers = new Map<number, { name: string; parts: string[] }>()

  for (const event of jsonObjects) {
    const eventType = typeof event.type === "string" ? event.type : ""
    const delta = event.delta
    const message = event.message

    // OpenAI Responses API：reasoning summary 事件单独路由到思考过程，
    // 避免和 output_text 一起落到 fallback 被当作正文。
    if (eventType.startsWith("response.reasoning") || eventType.startsWith("reasoning_summary")) {
      const text = collectTextFromUnknown(delta) || collectTextFromUnknown(event.part)
      if (text) reasoningParts.push(text)
      continue
    }

    if (eventType === "content_block_start") {
      const contentBlock = (event.content_block ?? null) as Record<string, unknown> | null
      if (!contentBlock) continue

      if (contentBlock.type === "text") {
        const text = collectTextFromUnknown(contentBlock)
        if (text) contentParts.push(text)
      } else if (contentBlock.type === "thinking") {
        const text = collectTextFromUnknown(contentBlock)
        if (text) reasoningParts.push(text)
      } else if (contentBlock.type === "tool_use") {
        const idx = typeof event.index === "number" ? event.index : 0
        const name =
          typeof contentBlock.name === "string" && contentBlock.name
            ? contentBlock.name
            : "tool"
        toolUseBuffers.set(idx, { name, parts: [] })
      }
      continue
    }

    if (eventType === "content_block_delta" && delta && typeof delta === "object") {
      const deltaRecord = delta as Record<string, unknown>
      if (deltaRecord.type === "text_delta" && typeof deltaRecord.text === "string") {
        contentParts.push(deltaRecord.text)
      } else if (
        deltaRecord.type === "thinking_delta" &&
        typeof deltaRecord.thinking === "string"
      ) {
        reasoningParts.push(deltaRecord.thinking)
      } else if (
        deltaRecord.type === "input_json_delta" &&
        typeof deltaRecord.partial_json === "string"
      ) {
        const idx = typeof event.index === "number" ? event.index : 0
        toolUseBuffers.get(idx)?.parts.push(deltaRecord.partial_json)
      }
      continue
    }

    if (eventType === "content_block_stop") {
      const idx = typeof event.index === "number" ? event.index : 0
      const buf = toolUseBuffers.get(idx)
      if (buf) {
        toolUseBuffers.delete(idx)
        let inputText = buf.parts.join("")
        try {
          inputText = JSON.stringify(JSON.parse(inputText), null, 2)
        } catch {
          // 参数片段拼不出合法 JSON（极少见，如上游截断）时保留原始拼接文本。
        }
        contentParts.push(`\n\n[Tool: ${buf.name}]\n${inputText}`)
      }
      continue
    }

    if (eventType === "message_start" && message && typeof message === "object") {
      const messageRecord = message as Record<string, unknown>
      const text = collectTextFromUnknown(messageRecord.content)
      if (text) contentParts.push(text)
      continue
    }

    if (Array.isArray(event.choices)) {
      for (const choice of event.choices as Array<Record<string, unknown>>) {
        const choiceDelta = choice?.delta
        if (typeof choiceDelta === "string") {
          contentParts.push(choiceDelta)
          continue
        }

        if (choiceDelta && typeof choiceDelta === "object") {
          const deltaRecord = choiceDelta as Record<string, unknown>

          // reasoning_content（deepseek/glm 等推理模型）/ reasoning 推理内容
          const reasoningContent = deltaRecord.reasoning_content ?? deltaRecord.reasoning
          if (typeof reasoningContent === "string" && reasoningContent) {
            reasoningParts.push(reasoningContent)
          }

          // content 普通内容
          const content = deltaRecord.content
          if (typeof content === "string" && content) {
            contentParts.push(content)
          }
          if (Array.isArray(content)) {
            const text = content.map((item) => collectTextFromUnknown(item)).join("")
            if (text) contentParts.push(text)
          }

          // tool_calls 工具调用
          const toolCalls = deltaRecord.tool_calls
          if (Array.isArray(toolCalls)) {
            for (const tc of toolCalls) {
              if (!tc || typeof tc !== "object") continue
              const tcRecord = tc as Record<string, unknown>
              const func = tcRecord.function
              if (func && typeof func === "object") {
                const funcRecord = func as Record<string, unknown>
                // 函数名只在第一次出现时添加
                if (typeof funcRecord.name === "string" && funcRecord.name) {
                  contentParts.push(`\n[Tool: ${funcRecord.name}]\n`)
                }
                // 参数逐块拼接
                if (typeof funcRecord.arguments === "string") {
                  contentParts.push(funcRecord.arguments)
                }
              }
            }
          }
        }
      }
      continue
    }

    const text = collectTextFromUnknown(event.output)
      || collectTextFromUnknown(event.content)
      || collectTextFromUnknown(delta)

    if (text) contentParts.push(text)
  }

  return {
    reasoning: reasoningParts.join("").trim(),
    content: contentParts.join("").trim(),
  }
}

export function getPayloadBytes(payload: string | null | undefined): number {
  const text = String(payload ?? "")
  return new TextEncoder().encode(text).length
}

function getSortableValue(item: ConsoleRequestListItem, sortKey: RequestSortKey): number | string {
  if (sortKey === "created_at") return item.created_at ?? 0
  if (sortKey === "response_status") return item.response_status ?? -1
  return getTotalTokens(item.response_usage, item.upstream_type)
}

export function sortRequests(
  requests: ConsoleRequestListItem[],
  sortKey: RequestSortKey,
  sortDirection: SortDirection,
): ConsoleRequestListItem[] {
  const copied = [...requests]
  copied.sort((left, right) => {
    const leftValue = getSortableValue(left, sortKey)
    const rightValue = getSortableValue(right, sortKey)

    let result = 0
    if (typeof leftValue === "string" && typeof rightValue === "string") {
      result = leftValue.localeCompare(rightValue, "zh-CN")
    } else {
      result = Number(leftValue) - Number(rightValue)
    }

    return sortDirection === "asc" ? result : -result
  })
  return copied
}
