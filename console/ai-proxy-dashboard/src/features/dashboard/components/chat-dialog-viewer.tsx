import { useCallback, useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { ChevronDown, ChevronRight, ChevronUp, Wrench } from "lucide-react"
import { Button } from "@/components/ui/button"
import { useIsMobile } from "@/hooks/use-is-mobile"
import { cn } from "@/lib/utils"

type ChatMessage = {
  role: string
  content: unknown
  name?: string
}

type ContentBlock = {
  type: string
  text?: string
  thinking?: string
  signature?: string
  name?: string
  id?: string
  input?: Record<string, unknown>
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
  user: "user",
  assistant: "assistant",
  tool: "tool",
}

// 聊天气泡样式：user 右对齐主色圆角泡、assistant 左对齐卡片圆角泡、system 居中小泡、
// 其余未知角色用等宽小卡片。圆角非对称（rounded-tr-md / rounded-tl-md）模拟气泡"尖角"。
function textBubbleClass(
  isUser: boolean,
  isAssistant: boolean,
  isSystem: boolean,
  isUnknown: boolean,
) {
  return cn(
    "w-full whitespace-pre-wrap break-words px-3.5 py-2 text-sm leading-relaxed [overflow-wrap:anywhere]",
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

function formatToolInput(input: Record<string, unknown> | undefined): string {
  if (!input) return ""
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
          {thinking}
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
  input?: Record<string, unknown>
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
            <div className="text-[10px] text-muted-foreground">
              {t("payload.toolCallId")}: {id}
            </div>
          )}
          {input && Object.keys(input).length > 0 && (
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
        <pre className="max-h-64 overflow-auto whitespace-pre-wrap break-words border-t border-inherit px-3 py-2 font-mono text-xs leading-relaxed text-muted-foreground">
          {text}
        </pre>
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
          <div
            key={key}
            className={textBubbleClass(isUser, isAssistant, isSystem, isUnknown)}
          >
            {block.text || ""}
          </div>
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
          const isSystem = role === "system"
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
                  "min-w-0 space-y-1.5",
                  isUser && "max-w-[85%] flex flex-col items-end",
                  isAssistant && "max-w-[85%] flex flex-col items-start",
                  isSystem && "max-w-[92%]",
                  isUnknown && "max-w-[92%]",
                )}
              >
                {typeof content === "string" ? (
                  <div
                    className={textBubbleClass(
                      isUser,
                      isAssistant,
                      isSystem,
                      isUnknown,
                    )}
                  >
                    {content || (
                      <span className="text-muted-foreground">(empty)</span>
                    )}
                  </div>
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
