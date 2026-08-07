import { useState } from "react"
import { useTranslation } from "react-i18next"
import { ChevronDown, ChevronRight } from "lucide-react"
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
import { Textarea } from "@/components/ui/textarea"
import { JsonViewer } from "@/components/ui/json-viewer"
import { formatBytes, getPayloadBytes, getPayloadText } from "@/features/dashboard/utils"
import { ChatDialogViewer } from "@/features/dashboard/components/chat-dialog-viewer"

type ChatMessage = {
  role: string
  content: unknown
  name?: string
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

  if (isValidJson && parsedJson && typeof parsedJson === "object") {
    const obj = parsedJson as Record<string, unknown>

    if (Array.isArray(obj.messages)) {
      messages = obj.messages as ChatMessage[]
    } else if (Array.isArray(obj.input)) {
      messages = obj.input as ChatMessage[]
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

    if (Array.isArray(obj.tools) && obj.tools.length > 0) {
      tools = obj.tools
    }
  }

  const hasMessages = messages !== null && messages.length > 0
  const hasSystem = systemBlocks !== null && systemBlocks.length > 0
  const hasTools = tools !== null

  type ViewMode = "chat" | "system" | "tools" | "json" | "raw"

  const defaultMode: ViewMode = hasMessages
    ? "chat"
    : hasSystem
      ? "system"
      : hasTools
        ? "tools"
        : "json"

  const [viewMode, setViewMode] = useState<ViewMode>(defaultMode)

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
        {isValidJson && (
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
              <Button
                variant={viewMode === "json" ? "default" : "outline"}
                size="sm"
                onClick={() => setViewMode("json")}
              >
                {t("payload.structured")}
              </Button>
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
              <Textarea
                readOnly
                value={payloadText}
                className="min-h-[24rem] w-full resize-none overflow-auto whitespace-pre-wrap break-all bg-background font-mono text-[11px] leading-5"
              />
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
            fn?.parameters ?? tool.input_schema ?? null

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
