import { useCallback, useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { Check, ChevronDown, ChevronRight, ChevronUp, Copy, Wrench } from "lucide-react"
import { Button } from "@/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { MarkdownRenderer } from "@/components/ui/markdown"
import { MarkdownCodeViewer } from "@/components/ui/markdown-code-viewer"
import { toast } from "@/components/ui/toast"
import { useIsMobile } from "@/hooks/use-is-mobile"
import { copyText } from "@/lib/clipboard"
import { cn } from "@/lib/utils"

export type ChatMessage = {
  role: string
  content: unknown
  name?: string
}

export type ContentBlock = {
  type: string
  text?: string
  thinking?: string
  signature?: string
  name?: string
  id?: string
  // Anthropic tool_use 是对象；OpenAI Responses API（Codex）的 custom_tool_call.input
  // 是字符串（如 exec 工具的 JS 代码），故同时兼容两种形态。
  input?: Record<string, unknown> | string
  content?: string | Array<{ type: string; text?: string }>
  tool_use_id?: string
  is_error?: boolean
  image_url?: { url: string }
  source?: unknown
}

export function extractMessageText(content: unknown): string {
  if (typeof content === "string") return content
  if (Array.isArray(content)) {
    const parts: string[] = []
    for (const p of content) {
      if (typeof p !== "object" || p === null) continue
      const part = p as Record<string, unknown>
      if (part.type === "text" && typeof part.text === "string") {
        parts.push(part.text)
      } else if (part.type === "image_url" || part.type === "image") {
        parts.push("[图片]")
      }
    }
    return parts.length > 0 ? parts.join("\n") : JSON.stringify(content)
  }
  if (content === null || content === undefined) return ""
  return JSON.stringify(content)
}

const ROLE_LABELS: Record<string, string> = {
  system: "system",
  // OpenAI 用 developer 角色承载系统级指令（Responses API / Codex 常见），按 system 样式渲染。
  developer: "developer",
  user: "user",
  assistant: "assistant",
  tool: "tool",
}

// 聊天气泡样式：user 右对齐主色圆角泡、assistant 左对齐卡片圆角泡、system 居中小泡、
// 其余未知角色用等宽小卡片。圆角非对称（rounded-tr-md / rounded-tl-md）模拟气泡"尖角"。
// wrap=true（默认）保留 whitespace-pre-wrap，适合纯文本；Markdown 渲染时传 false，
// 让 react-markdown 自己控制换行（否则 pre-wrap 会让 Markdown 的软换行变成硬换行）。
function textBubbleClass(
  isUser: boolean,
  isAssistant: boolean,
  isSystem: boolean,
  isUnknown: boolean,
  wrap = true,
) {
  return cn(
    "w-full px-3.5 py-2 text-sm leading-relaxed [overflow-wrap:anywhere]",
    wrap && "whitespace-pre-wrap break-words",
    isUser &&
      "rounded-2xl rounded-tr-md border border-primary/25 bg-primary/12 text-foreground",
    isAssistant &&
      "rounded-2xl rounded-tl-md border border-border/60 bg-muted text-foreground shadow-sm",
    isSystem &&
      "rounded-xl bg-secondary/40 text-xs italic text-secondary-foreground",
    isUnknown &&
      "rounded-lg border-l-2 border-secondary/40 bg-secondary/10 font-mono text-xs text-secondary-foreground",
  )
}

// 文本气泡右上角菜单：切换 渲染 / 源码（CodeMirror），以及复制原始 Markdown 文本。
function BubbleMenu({
  text,
  mode,
  setMode,
}: {
  text: string
  mode: "rendered" | "source"
  setMode: (m: "rendered" | "source") => void
}) {
  const { t } = useTranslation()
  const handleCopy = () => {
    copyText(text).then((ok) =>
      ok ? toast.success(t("common.copied")) : toast.error(t("common.copyFailed")),
    )
  }
  return (
    <div className="absolute right-0 top-0 z-10">
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            aria-label={t("payload.mdMenu")}
            className="flex h-4 w-4 items-center justify-center rounded text-muted-foreground/50 transition-colors hover:bg-foreground/10 hover:text-foreground"
          >
            <ChevronDown className="h-3 w-3" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" sideOffset={4} className="min-w-[9rem]">
          <DropdownMenuItem onClick={() => setMode("rendered")}>
            <Check
              className={cn("h-3.5 w-3.5", mode === "rendered" ? "opacity-100" : "opacity-0")}
            />
            {t("payload.mdRendered")}
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => setMode("source")}>
            <Check
              className={cn("h-3.5 w-3.5", mode === "source" ? "opacity-100" : "opacity-0")}
            />
            {t("payload.mdSource")}
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem onClick={handleCopy}>
            <Copy className="h-3.5 w-3.5" />
            {t("common.copy")}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  )
}

// 文本气泡内容：默认 Markdown 渲染（安���），可切到 CodeMirror 源码视图；右上角菜单。
function MessageText({ text }: { text: string }) {
  const [mode, setMode] = useState<"rendered" | "source">("rendered")
  return (
    <div className="relative">
      <BubbleMenu text={text} mode={mode} setMode={setMode} />
      {/* pr-5 给右上角菜单按钮留位，避免第一行文字压到按钮下 */}
      <div className="pr-5">
        {mode === "rendered" ? (
          <MarkdownRenderer text={text} />
        ) : (
          <MarkdownCodeViewer value={text} />
        )}
      </div>
    </div>
  )
}

// 统一的文本气泡：user / assistant 文本走 Markdown 渲染 + 菜单；system / unknown 保持纯文本。
function TextBubble({
  text,
  isUser,
  isAssistant,
  isSystem,
  isUnknown,
}: {
  text: string
  isUser: boolean
  isAssistant: boolean
  isSystem: boolean
  isUnknown: boolean
}) {
  const enableMd = (isUser || isAssistant) && text.length > 0
  return (
    <div
      className={cn(
        textBubbleClass(isUser, isAssistant, isSystem, isUnknown, !enableMd),
        enableMd && "relative",
      )}
    >
      {enableMd ? (
        <MessageText text={text} />
      ) : (
        text || <span className="text-muted-foreground">(empty)</span>
      )}
    </div>
  )
}

function formatToolInput(input: Record<string, unknown> | string | undefined): string {
  if (input === undefined || input === null) return ""
  // 字符串 input（Codex exec 工具的 JS 代码、function_call 的 JSON 参数串）整段直接展示，
  // 不按 key 截断，避免把一整段代码切碎。
  if (typeof input === "string") return input
  const lines: string[] = []
  for (const [key, value] of Object.entries(input)) {
    const str = typeof value === "string" ? value : JSON.stringify(value)
    if (str.length > 200) {
      lines.push(`${key}: ${str.slice(0, 200)}...`)
    } else {
      lines.push(`${key}: ${str}`)
    }
  }
  return lines.join("\n")
}

function hasToolInput(input: Record<string, unknown> | string | undefined): boolean {
  if (typeof input === "string") return input.length > 0
  return !!input && Object.keys(input).length > 0
}

function ThinkingBlock({
  thinking,
}: {
  thinking: string
}) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)

  return (
    <div className="border-l-2 border-amber-400/50 bg-amber-50/30 dark:bg-amber-950/10">
      <button
        type="button"
        onClick={() => setOpen(!open)}
        className="flex w-full items-center gap-1.5 px-3 py-1.5 text-xs font-medium text-amber-700 dark:text-amber-400"
      >
        {open ? (
          <ChevronDown className="h-3 w-3" />
        ) : (
          <ChevronRight className="h-3 w-3" />
        )}
        {t("payload.thinking")}
      </button>
      {open && (
        <div className="whitespace-pre-wrap break-words border-t border-amber-400/15 px-3 py-2 text-xs leading-relaxed text-amber-800/80 dark:text-amber-300/80">
          {/* Codex/Responses API 的 reasoning 通常是加密的（encrypted_content），summary 为空， */}
          {/* 此时展示占位说明，避免出现空白折叠区。 */}
          {thinking || t("payload.reasoningHidden")}
        </div>
      )}
    </div>
  )
}

function ToolUseBlock({
  name,
  id,
  input,
}: {
  name: string
  id: string
  input?: Record<string, unknown> | string
}) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const shortId = id.length > 16 ? `${id.slice(0, 8)}...${id.slice(-4)}` : id

  return (
    <div className="border-l-2 border-blue-400/50 bg-blue-50/30 dark:bg-blue-950/10">
      <button
        type="button"
        onClick={() => setOpen(!open)}
        className="flex w-full items-center gap-1.5 px-3 py-1.5 text-xs"
      >
        {open ? (
          <ChevronDown className="h-3 w-3 shrink-0 text-muted-foreground" />
        ) : (
          <ChevronRight className="h-3 w-3 shrink-0 text-muted-foreground" />
        )}
        <Wrench className="h-3 w-3 shrink-0 text-blue-600 dark:text-blue-400" />
        <span className="truncate font-mono font-medium text-blue-700 dark:text-blue-400">
          {name}
        </span>
        <span className="shrink-0 text-[10px] text-muted-foreground">
          {id ? `#${shortId}` : ""}
        </span>
      </button>
      {open && (
        <div className="space-y-1 border-t border-blue-400/15 px-3 py-2">
          {id && (
            <div className="break-all font-mono text-[10px] text-muted-foreground">
              {t("payload.toolCallId")}: {id}
            </div>
          )}
          {hasToolInput(input) && (
            <pre className="whitespace-pre-wrap break-words font-mono text-xs leading-relaxed text-muted-foreground">
              {formatToolInput(input)}
            </pre>
          )}
        </div>
      )}
    </div>
  )
}

function ToolResultBlock({
  content,
  isError,
  toolUseId,
}: {
  content: unknown
  isError?: boolean
  toolUseId?: string
}) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const text = typeof content === "string" ? content : JSON.stringify(content, null, 2)
  const shortId = toolUseId
    ? toolUseId.length > 16
      ? `${toolUseId.slice(0, 8)}...${toolUseId.slice(-4)}`
      : toolUseId
    : null

  return (
    <div
      className={cn(
        "border-l-2",
        isError
          ? "border-red-400/50 bg-red-50/30 dark:bg-red-950/10"
          : "border-green-400/50 bg-green-50/30 dark:bg-green-950/10",
      )}
    >
      <button
        type="button"
        onClick={() => setOpen(!open)}
        className="flex w-full items-center gap-1.5 px-3 py-1.5 text-xs"
      >
        {open ? (
          <ChevronDown className="h-3 w-3 shrink-0 text-muted-foreground" />
        ) : (
          <ChevronRight className="h-3 w-3 shrink-0 text-muted-foreground" />
        )}
        <span
          className={cn(
            "shrink-0 font-medium",
            isError
              ? "text-red-700 dark:text-red-400"
              : "text-green-700 dark:text-green-400",
          )}
        >
          {isError ? t("payload.toolResultError") : t("payload.toolResult")}
        </span>
        {shortId && (
          <span className="truncate font-mono text-[10px] text-muted-foreground">
            #{shortId}
          </span>
        )}
        <span className="truncate text-muted-foreground/60">
          {text.split("\n")[0].slice(0, 80)}
        </span>
      </button>
      {open && (
        <div className="border-t border-inherit px-3 py-2">
          {toolUseId && (
            <div className="mb-1.5 break-all font-mono text-[10px] text-muted-foreground">
              {t("payload.toolCallId")}: {toolUseId}
            </div>
          )}
          <pre className="max-h-64 overflow-auto whitespace-pre-wrap break-words font-mono text-xs leading-relaxed text-muted-foreground">
            {text}
          </pre>
        </div>
      )}
    </div>
  )
}

function renderMessageBlocks(
  blocks: ContentBlock[],
  isUser: boolean,
  isAssistant: boolean,
  isSystem: boolean,
  isUnknown: boolean,
  t: (key: string) => string,
) {
  return blocks.map((block, i) => {
    const key = `${block.type}-${i}`
    switch (block.type) {
      case "text":
        return (
          <TextBubble
            key={key}
            text={block.text || ""}
            isUser={isUser}
            isAssistant={isAssistant}
            isSystem={isSystem}
            isUnknown={isUnknown}
          />
        )
      case "thinking":
        return (
          <ThinkingBlock
            key={key}
            thinking={block.thinking || ""}
          />
        )
      case "tool_use":
        return (
          <ToolUseBlock
            key={key}
            name={block.name || "unknown"}
            id={block.id || ""}
            input={block.input}
          />
        )
      case "tool_result":
        return (
          <ToolResultBlock
            key={key}
            content={block.content}
            isError={block.is_error}
            toolUseId={block.tool_use_id}
          />
        )
      case "image":
      case "image_url":
        return (
          <div
            key={key}
            className="text-xs italic text-muted-foreground"
          >
            [{t("payload.image")}]
          </div>
        )
      default:
        return (
          <div key={key} className="text-xs text-muted-foreground">
            [{block.type}]
          </div>
        )
    }
  })
}

export function ChatDialogViewer({ messages }: { messages: ChatMessage[] }) {
  const { t } = useTranslation()
  const isMobile = useIsMobile()
  const scrollRef = useRef<HTMLDivElement>(null)
  const [visibleCount, setVisibleCount] = useState(10)
  const prevScrollHeightRef = useRef(0)

  const total = messages.length
  const visible = messages.slice(Math.max(0, total - visibleCount))
  const hasMore = visibleCount < total

  const scrollToBottom = useCallback(() => {
    const el = scrollRef.current
    if (el) {
      el.scrollTop = el.scrollHeight
    }
  }, [])

  useEffect(() => {
    // 仅桌面端走内部滚动；移动端随 Sheet 页面滚动，无需手动定位。
    if (isMobile) return
    if (prevScrollHeightRef.current > 0 && scrollRef.current) {
      scrollRef.current.scrollTop =
        scrollRef.current.scrollHeight - prevScrollHeightRef.current
    } else {
      scrollToBottom()
    }
  }, [visibleCount, scrollToBottom, isMobile])

  const handleLoadMore = useCallback(() => {
    if (scrollRef.current) {
      prevScrollHeightRef.current = scrollRef.current.scrollHeight
    }
    setVisibleCount((prev) => Math.min(prev + 10, total))
  }, [total])

  // 桌面端：固定高度的卡片框 + 内部滚动；移动端：去掉框与内部滚动，随页面流式排布，
  // 这样在移动端更像一条可自然上下滑动的聊天记录，而不是嵌套滚动框。
  return (
    <div
      className={
        isMobile
          ? "flex flex-col"
          : "flex min-h-[24rem] flex-col border bg-muted/30"
      }
    >
      {hasMore && (
        <div
          className={cn(
            "flex justify-center px-4 py-2",
            !isMobile && "border-b",
          )}
        >
          <Button variant="ghost" size="sm" onClick={handleLoadMore}>
            <ChevronUp data-icon="inline-start" />
            {t("payload.loadMore", { count: total - visibleCount })}
          </Button>
        </div>
      )}
      <div
        ref={scrollRef}
        className={
          isMobile ? "space-y-3 py-1" : "flex-1 space-y-3 overflow-auto px-4 py-4"
        }
      >
        {visible.map((msg, idx) => {
          const role = msg.role || "unknown"
          const isUser = role === "user"
          const isAssistant = role === "assistant"
          const isSystem = role === "system" || role === "developer"
          const isUnknown = !isUser && !isAssistant && !isSystem

          const content = msg.content
          const isArrayContent = Array.isArray(content)

          return (
            <div
              key={`${total - visibleCount + idx}-${role}`}
              className={cn(
                "flex flex-col gap-0.5",
                isUser && "items-end",
                isAssistant && "items-start",
                isSystem && "items-center",
                isUnknown && "items-start",
              )}
            >
              <span className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
                {ROLE_LABELS[role] || role}
                {msg.name ? ` (${msg.name})` : ""}
              </span>
              <div
                className={cn(
                  // [&>*]:min-w-0 + max-w-full：把每个气泡/卡片都钳制在本包装列宽度���。
                  // 工具调用/工具结果卡片（border-l-2）头部是 flex 行，内含 truncate 的
                  // 长工具名（如 mcp__plugin_chrome-devtools-mcp_chrome-devtools__take_screenshot）。
                  // 卡片作为 flex-col 包装列里 align-self:start 的子项，默认按 max-content 撑宽，
                  // 长工具名会把卡片顶出包装列、顶出视口（移动端右侧被 overflow:hidden 裁掉看不到），
                  // truncate 也因此失效。给所有直接子项 max-w-full(+min-w-0) 后，卡片回缩到列宽，
                  // 内层 truncate 才正常生效；短内容的卡片仍按内容窄排，气泡(w-full)无视觉变化。
                  "min-w-0 space-y-1.5 [&>*]:min-w-0 [&>*]:max-w-full",
                  isUser && "max-w-[85%] flex flex-col items-end",
                  isAssistant && "max-w-[85%] flex flex-col items-start",
                  isSystem && "max-w-[92%]",
                  isUnknown && "max-w-[92%]",
                )}
              >
                {typeof content === "string" ? (
                  <TextBubble
                    text={content}
                    isUser={isUser}
                    isAssistant={isAssistant}
                    isSystem={isSystem}
                    isUnknown={isUnknown}
                  />
                ) : isArrayContent ? (
                  renderMessageBlocks(
                    content as ContentBlock[],
                    isUser,
                    isAssistant,
                    isSystem,
                    isUnknown,
                    t,
                  )
                ) : (
                  <div
                    className={cn(
                      textBubbleClass(
                        isUser,
                        isAssistant,
                        isSystem,
                        isUnknown,
                      ),
                    )}
                  >
                    {content === null || content === undefined ? (
                      <span className="text-muted-foreground">(empty)</span>
                    ) : (
                      JSON.stringify(content)
                    )}
                  </div>
                )}
              </div>
            </div>
          )
        })}
        {visible.length === 0 && (
          <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
            {t("payload.noMessages")}
          </div>
        )}
      </div>
    </div>
  )
}
