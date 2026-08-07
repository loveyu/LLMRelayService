import { useCallback, useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { ChevronDown, ChevronRight, ChevronUp, Wrench } from "lucide-react"
import { Button } from "@/components/ui/button"
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
            className={cn(
              "w-full whitespace-pre-wrap break-words px-4 py-2.5 text-sm leading-relaxed",
              isUser &&
                "border-r-2 border-primary bg-primary/10 text-foreground",
              isAssistant &&
                "border-l-2 border-transparent bg-muted text-foreground",
              isSystem &&
                "bg-secondary/15 text-xs italic text-secondary-foreground",
              isUnknown &&
                "border-l-2 border-secondary/40 bg-secondary/10 font-mono text-xs text-secondary-foreground",
            )}
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
    if (prevScrollHeightRef.current > 0 && scrollRef.current) {
      scrollRef.current.scrollTop =
        scrollRef.current.scrollHeight - prevScrollHeightRef.current
    } else {
      scrollToBottom()
    }
  }, [visibleCount, scrollToBottom])

  const handleLoadMore = useCallback(() => {
    if (scrollRef.current) {
      prevScrollHeightRef.current = scrollRef.current.scrollHeight
    }
    setVisibleCount((prev) => Math.min(prev + 10, total))
  }, [total])

  return (
    <div className="flex min-h-[24rem] flex-col border bg-muted/30">
      {hasMore && (
        <div className="flex justify-center border-b px-4 py-2">
          <Button variant="ghost" size="sm" onClick={handleLoadMore}>
            <ChevronUp data-icon="inline-start" />
            {t("payload.loadMore", { count: total - visibleCount })}
          </Button>
        </div>
      )}
      <div ref={scrollRef} className="flex-1 space-y-3 overflow-auto px-4 py-4">
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
                "flex flex-col gap-1",
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
                  "max-w-[85%] space-y-1.5",
                  isUser && "flex flex-col items-end",
                  isAssistant && "flex flex-col items-start",
                  isSystem && "max-w-[92%]",
                  isUnknown && "max-w-[92%]",
                )}
              >
                {typeof content === "string" ? (
                  <div
                    className={cn(
                      "w-full whitespace-pre-wrap break-words px-4 py-2.5 text-sm leading-relaxed",
                      isUser &&
                        "border-r-2 border-primary bg-primary/10 text-foreground",
                      isAssistant &&
                        "border-l-2 border-transparent bg-muted text-foreground",
                      isSystem &&
                        "bg-secondary/15 text-xs italic text-secondary-foreground",
                      isUnknown &&
                        "border-l-2 border-secondary/40 bg-secondary/10 font-mono text-xs text-secondary-foreground",
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
                      "w-full whitespace-pre-wrap break-words px-4 py-2.5 text-sm leading-relaxed",
                      isUnknown &&
                        "border-l-2 border-secondary/40 bg-secondary/10 font-mono text-xs text-secondary-foreground",
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
