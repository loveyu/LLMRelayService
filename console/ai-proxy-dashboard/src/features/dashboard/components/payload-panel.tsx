import { useState } from "react"
import { useTranslation } from "react-i18next"
import { ChevronDown, ChevronRight, Copy, ExternalLink } from "lucide-react"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyTitle,
} from "@/components/ui/empty"
import { Separator } from "@/components/ui/separator"
import { toast } from "@/components/ui/toast"
import { JsonViewer } from "@/components/ui/json-viewer"
import { JsonCodeViewer } from "@/components/ui/json-code-viewer"
import { copyText } from "@/lib/clipboard"
import { formatBytes, getPayloadBytes, getPayloadText } from "@/features/dashboard/utils"
import { ChatDialogViewer, type ChatMessage, type ContentBlock } from "@/features/dashboard/components/chat-dialog-viewer"

// OpenAI Responses API（Codex CLI / ChatGPT Codex 走 /openai/v1/responses）的请求体用 `input`
// 数组承载对话，每个条目用 `type` 字段区分：message / custom_tool_call /
// custom_tool_call_output / reasoning / additional_tools / function_call(_output) 等，
// 与 Anthropic 及 OpenAI Chat Completions 的 messages+content-block 结构完全不同。
// 这里把它们归一化成 ChatDialogViewer 已能渲染的 ChatMessage + ContentBlock 词表，
// 不引入新的渲染分支，保持对话列表视图统一。

// Responses API input 数组里 content / output 的单个 part。
type ResponsesPart = {
  type?: string
  text?: string
  refusal?: string
}

// Responses API input 数组的单个条目（只列归一化用到的字段）。
type ResponsesItem = {
  type?: string
  role?: string
  id?: string
  content?: unknown
  name?: string
  call_id?: string
  input?: unknown
  output?: unknown
  summary?: Array<{ type?: string; text?: string }>
  encrypted_content?: string
  tools?: unknown[]
}

const RESPONSES_ITEM_TYPES = new Set<string>([
  "message",
  "function_call",
  "function_call_output",
  "custom_tool_call",
  "custom_tool_call_output",
  "reasoning",
  "additional_tools",
  "computer_call",
  "computer_call_output",
  "web_search_call",
  "file_search_call",
  "image_generation_call",
])

function isResponsesApiInput(items: unknown[]): boolean {
  return items.some(
    (i) =>
      i !== null &&
      typeof i === "object" &&
      RESPONSES_ITEM_TYPES.has((i as ResponsesItem).type ?? ""),
  )
}

// additional_tools 可能是 namespace 嵌套（Codex 的 functions/collaboration 命名空间），
// 拍平成扁平工具列表交给 ToolsView。
function flattenResponsesTools(tools: unknown[]): unknown[] {
  const out: unknown[] = []
  for (const t of tools) {
    if (t !== null && typeof t === "object") {
      const obj = t as Record<string, unknown>
      if (obj.type === "namespace" && Array.isArray(obj.tools)) {
        out.push(...flattenResponsesTools(obj.tools))
      } else {
        out.push(obj)
      }
    }
  }
  return out
}

// message.content（input_text/output_text/text/refusal/image）→ ContentBlock[]
function normalizeResponsesContent(content: unknown): ContentBlock[] | string {
  if (typeof content === "string") return content
  if (!Array.isArray(content)) return []
  const blocks: ContentBlock[] = []
  for (const part of content) {
    if (typeof part === "string") {
      blocks.push({ type: "text", text: part })
      continue
    }
    if (part !== null && typeof part === "object") {
      const p = part as ResponsesPart
      if (p.type === "input_text" || p.type === "output_text" || p.type === "text") {
        if (typeof p.text === "string") blocks.push({ type: "text", text: p.text })
      } else if (p.type === "refusal" && typeof p.refusal === "string") {
        blocks.push({ type: "text", text: p.refusal })
      } else if (p.type === "image_url" || p.type === "input_image" || p.type === "image") {
        blocks.push({ type: "image" })
      } else {
        // 未知 part：保留 JSON 文本，避免静默丢数据
        blocks.push({ type: "text", text: JSON.stringify(part) })
      }
    }
  }
  return blocks
}

// tool 输出（string 或 part 数组）→ 单段文本，交给 ToolResultBlock。
function normalizeResponsesOutput(output: unknown): string {
  if (typeof output === "string") return output
  if (Array.isArray(output)) {
    const texts: string[] = []
    for (const part of output) {
      if (typeof part === "string") {
        texts.push(part)
      } else if (part !== null && typeof part === "object") {
        const p = part as ResponsesPart
        const txt = p.text ?? p.refusal
        if (typeof txt === "string") texts.push(txt)
        else texts.push(JSON.stringify(part))
      }
    }
    return texts.join("\n")
  }
  if (output === null || output === undefined) return ""
  return JSON.stringify(output)
}

// Responses API input 数组 → { messages, tools }，复用 viewer 词表：
// - message(user/assistant)        → 文本气泡
// - message(developer/system)      → system 样式气泡（系统级指令）
// - custom_tool_call/function_call → tool_use 卡片（assistant 侧，input 保留字符串）
// - custom_tool_call_output 等     → tool_result 卡片（user 侧）
// - reasoning                      → thinking 折叠块（summary 为空时由占位说明）
// - additional_tools               → 不进对话列表，折叠进 Tools tab
function normalizeResponsesApiInput(items: ResponsesItem[]): {
  messages: ChatMessage[]
  tools: unknown[]
} {
  const messages: ChatMessage[] = []
  const tools: unknown[] = []
  for (const item of items) {
    if (item === null || typeof item !== "object") continue
    switch (item.type) {
      case "additional_tools": {
        if (Array.isArray(item.tools)) tools.push(...flattenResponsesTools(item.tools))
        break
      }
      case "message": {
        messages.push({
          role: item.role || "unknown",
          content: normalizeResponsesContent(item.content),
        })
        break
      }
      case "custom_tool_call":
      case "function_call": {
        messages.push({
          role: "assistant",
          content: [
            {
              type: "tool_use",
              id: item.call_id || item.id || "",
              name: item.name || "unknown",
              input: item.input,
            } as ContentBlock,
          ],
        })
        break
      }
      case "custom_tool_call_output":
      case "function_call_output": {
        messages.push({
          role: "user",
          content: [
            {
              type: "tool_result",
              tool_use_id: item.call_id || item.id || "",
              content: normalizeResponsesOutput(item.output),
            } as ContentBlock,
          ],
        })
        break
      }
      case "reasoning": {
        const summary = Array.isArray(item.summary) ? item.summary : []
        const text = summary
          .map((s) => (s && typeof s.text === "string" ? s.text : ""))
          .filter(Boolean)
          .join("\n")
        messages.push({
          role: "assistant",
          content: [{ type: "thinking", thinking: text } as ContentBlock],
        })
        break
      }
      default: {
        // 未知条目类型：渲染成文本块，保证可见、不丢数据
        messages.push({
          role: "unknown",
          content: [{ type: "text", text: JSON.stringify(item) }],
        })
        break
      }
    }
  }
  return { messages, tools }
}

export function PayloadPanel({
  title,
  payload,
  truncated,
}: {
  title: string
  payload: string | null | undefined
  truncated: boolean
}) {
  const { t } = useTranslation()
  const payloadText = getPayloadText(payload)
  const payloadBytes = getPayloadBytes(payload)

  let parsedJson: unknown = null
  let isValidJson = false

  if (payloadText) {
    try {
      parsedJson = JSON.parse(payloadText)
      isValidJson = true
    } catch {
      isValidJson = false
    }
  }

  let messages: ChatMessage[] | null = null
  let systemBlocks: Array<{ text: string }> | null = null
  let tools: unknown = null
  // 从 Responses API additional_tools 条目里抽出���工具，稍后与顶层 tools 合并。
  let extractedTools: unknown[] | null = null

  if (isValidJson && parsedJson && typeof parsedJson === "object") {
    const obj = parsedJson as Record<string, unknown>

    if (Array.isArray(obj.messages)) {
      messages = obj.messages as ChatMessage[]
    } else if (Array.isArray(obj.input)) {
      const inputArr = obj.input as ResponsesItem[]
      if (isResponsesApiInput(inputArr)) {
        const norm = normalizeResponsesApiInput(inputArr)
        messages = norm.messages
        if (norm.tools.length > 0) extractedTools = norm.tools
      } else {
        // 非 Responses API 的 input 数组：保留旧行为，原样当作 messages。
        messages = inputArr as unknown as ChatMessage[]
      }
    }

    if (typeof obj.system === "string") {
      systemBlocks = [{ text: obj.system }]
    } else if (Array.isArray(obj.system)) {
      systemBlocks = (obj.system as Array<Record<string, unknown>>)
        .filter(
          (block): block is { type: string; text: string } =>
            block.type === "text" && typeof block.text === "string",
        )
        .map(({ text }) => ({ text }))
      if (systemBlocks.length === 0) {
        systemBlocks = [{ text: JSON.stringify(obj.system) }]
      }
    } else if (typeof obj.instructions === "string") {
      systemBlocks = [{ text: obj.instructions }]
    }

    const topTools =
      Array.isArray(obj.tools) && obj.tools.length > 0 ? obj.tools : null
    if (topTools || (extractedTools && extractedTools.length > 0)) {
      tools = [...(topTools ?? []), ...(extractedTools ?? [])]
    }
  }

  const hasMessages = messages !== null && messages.length > 0
  const hasSystem = systemBlocks !== null && systemBlocks.length > 0
  const hasTools = tools !== null

  type ViewMode = "chat" | "system" | "tools" | "json" | "raw"

  // response_payload 这类 SSE 文本流不是合法 JSON，JSON.parse 失败时直接落到「原始」
  // 视图，否则面板会整块空白（连 tab 都不渲染）。
  const defaultMode: ViewMode = !isValidJson
    ? "raw"
    : hasMessages
      ? "chat"
      : hasSystem
        ? "system"
        : hasTools
          ? "tools"
          : "json"

  const [viewMode, setViewMode] = useState<ViewMode>(defaultMode)

  // Blob：把格式化 JSON 写成 blob: 链接并在新窗口打开，便于全屏查看 / 单独分析。
  const handleOpenBlob = () => {
    const blob = new Blob([payloadText], { type: "application/json" })
    const url = URL.createObjectURL(blob)
    const win = window.open(url, "_blank")
    if (!win) {
      URL.revokeObjectURL(url)
      toast.error(t("payload.openBlobBlocked"))
      return
    }
    // 新窗口加载 blob 需要一点时间，延迟回收 object URL（页面卸载时也会被 GC）。
    window.setTimeout(() => URL.revokeObjectURL(url), 30000)
  }

  // 复制：拷贝原始未格式化的 payload（直接取 prop，不经过 getPayloadText 的 2-space 重排）。
  const handleCopyRaw = () => {
    copyText(payload ?? "").then((ok) =>
      ok ? toast.success(t("common.copied")) : toast.error(t("common.copyFailed")),
    )
  }

  return (
    <Card size="sm">
      <CardHeader className="border-b pb-4">
        <div className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
          <div className="space-y-1">
            <CardTitle>{title}</CardTitle>
            <CardDescription>
              {payloadText ? t("payload.supportCopyView") : t("payload.noContent")}
            </CardDescription>
          </div>
          <div className="flex items-center gap-2">
            <Button variant="ghost" size="sm" disabled className="pointer-events-none opacity-100">
              {truncated ? t("payload.truncated") : t("payload.fullRetained")}
            </Button>
            <Button variant="ghost" size="sm" disabled className="pointer-events-none opacity-100">
              {formatBytes(payloadBytes)}
            </Button>
          </div>
        </div>
        {payloadText && (
          <>
            <Separator className="my-3" />
            <div className="flex flex-wrap gap-1">
              {hasMessages && (
                <Button
                  variant={viewMode === "chat" ? "default" : "outline"}
                  size="sm"
                  onClick={() => setViewMode("chat")}
                >
                  {t("payload.chat")}
                </Button>
              )}
              {hasSystem && (
                <Button
                  variant={viewMode === "system" ? "default" : "outline"}
                  size="sm"
                  onClick={() => setViewMode("system")}
                >
                  {t("payload.systemTab")}
                </Button>
              )}
              {hasTools && (
                <Button
                  variant={viewMode === "tools" ? "default" : "outline"}
                  size="sm"
                  onClick={() => setViewMode("tools")}
                >
                  {t("payload.toolsTab")}
                </Button>
              )}
              {isValidJson && (
                <Button
                  variant={viewMode === "json" ? "default" : "outline"}
                  size="sm"
                  onClick={() => setViewMode("json")}
                >
                  {t("payload.structured")}
                </Button>
              )}
              <Button
                variant={viewMode === "raw" ? "default" : "outline"}
                size="sm"
                onClick={() => setViewMode("raw")}
              >
                {t("payload.raw")}
              </Button>
            </div>
          </>
        )}
      </CardHeader>
      <CardContent className="pt-4">
        {payloadText ? (
          <>
            {viewMode === "chat" && messages && (
              <ChatDialogViewer messages={messages} />
            )}
            {viewMode === "system" && systemBlocks && (
              <SystemPromptViewer blocks={systemBlocks} />
            )}
            {viewMode === "tools" && tools && (
              <ToolsView tools={tools as Array<Record<string, unknown>>} />
            )}
            {viewMode === "json" && isValidJson && (
              <div className="min-h-[24rem] w-full overflow-auto border bg-background p-4">
                <JsonViewer data={parsedJson} defaultExpanded />
              </div>
            )}
            {viewMode === "raw" && (
              <div className="space-y-2">
                {/* 操作栏：位于语法高亮之上 —— blob(新窗口打开格式化 JSON) + 复制(原始未格式化 JSON) */}
                <div className="flex flex-wrap items-center gap-2">
                  <Button variant="outline" size="sm" onClick={handleOpenBlob}>
                    <ExternalLink data-icon="inline-start" className="h-3.5 w-3.5" />
                    {t("payload.openBlob")}
                  </Button>
                  <Button variant="outline" size="sm" onClick={handleCopyRaw}>
                    <Copy data-icon="inline-start" className="h-3.5 w-3.5" />
                    {t("payload.copyRaw")}
                  </Button>
                </div>
                <JsonCodeViewer value={payloadText} height="24rem" />
              </div>
            )}
          </>
        ) : (
          <Empty className="border-border/70">
            <EmptyHeader>
              <EmptyTitle>{t("payload.emptyTitle")}</EmptyTitle>
              <EmptyDescription>
                {t("payload.emptyDescription")}
              </EmptyDescription>
            </EmptyHeader>
          </Empty>
        )}
      </CardContent>
    </Card>
  )
}

function ToolsView({ tools }: { tools: Array<Record<string, unknown>> }) {
  const { t } = useTranslation()

  return (
    <div className="min-h-[24rem] border bg-background p-4">
      <div className="mb-3 text-xs text-muted-foreground">
        {t("payload.toolsCount", { count: tools.length })}
      </div>
      <div className="space-y-1">
        {tools.map((tool, i) => {
          const fn =
            tool.function &&
            typeof tool.function === "object"
              ? (tool.function as Record<string, unknown>)
              : null
          const name = String(
            fn?.name ?? tool.name ?? `tool_${i + 1}`,
          )
          const rawDesc = fn?.description ?? tool.description
          const description = typeof rawDesc === "string" ? rawDesc : ""
          const schema =
            fn?.parameters ?? tool.input_schema ?? tool.parameters ?? null

          return (
            <ToolRow
              key={`${name}-${i}`}
              name={name}
              description={description}
              schema={schema}
            />
          )
        })}
      </div>
    </div>
  )
}

function ToolRow({
  name,
  description,
  schema,
}: {
  name: string
  description: string
  schema: unknown
}) {
  const [open, setOpen] = useState(false)

  return (
    <div>
      <button
        type="button"
        onClick={() => setOpen(!open)}
        className="flex w-full items-center gap-2 px-3 py-2 text-left hover:bg-muted/50"
      >
        {open ? (
          <ChevronDown className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        ) : (
          <ChevronRight className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        )}
        <span className="truncate font-mono text-sm font-medium text-foreground">
          {name}
        </span>
      </button>
      {open && (
        <>
          {description && (
            <div className="border-t px-6 py-3 text-sm leading-relaxed text-muted-foreground whitespace-pre-wrap break-words">
              {description}
            </div>
          )}
          {schema != null && (
            <div className={`${description ? "" : "border-t"} bg-muted/20 px-6 py-3`}>
              <div className="overflow-auto">
                <JsonViewer data={schema} defaultExpanded />
              </div>
            </div>
          )}
        </>
      )}
    </div>
  )
}

function SystemPromptViewer({
  blocks,
}: {
  blocks: Array<{ text: string }>
}) {
  return (
    <div className="min-h-[24rem] border bg-background p-4">
      {blocks.map((block, i) => (
        <SystemBlock key={i} text={block.text} index={i} total={blocks.length} />
      ))}
    </div>
  )
}

function SystemBlock({
  text,
  index,
  total,
}: {
  text: string
  index: number
  total: number
}) {
  const lines = text.split("\n")
  const headingMatch = lines[0]?.match(/^#{1,4}\s+(.+)/)

  return (
    <div>
      <div className="border-b px-1 py-2">
        <span className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
          {headingMatch ? headingMatch[1] : `block ${index + 1}/${total}`}
        </span>
      </div>
      <div className="max-h-[36rem] overflow-auto py-3">
        <div className="whitespace-pre-wrap break-words font-mono text-xs leading-relaxed text-foreground">
          {text}
        </div>
      </div>
    </div>
  )
}
