import { useState } from "react"
import { useTranslation } from "react-i18next"
import { ChevronDown, ChevronRight, Copy } from "lucide-react"

import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
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
import { ScrollArea } from "@/components/ui/scroll-area"
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@/components/ui/tabs"
import { Textarea } from "@/components/ui/textarea"
import { toast } from "@/components/ui/toast"
import { JsonCodeViewer } from "@/components/ui/json-code-viewer"
import { MarkdownCodeViewer } from "@/components/ui/markdown-code-viewer"
import { MarkdownRenderer } from "@/components/ui/markdown"
import { copyText } from "@/lib/clipboard"
import { useIsMobile } from "@/hooks/use-is-mobile"
import { DetailMetricTable } from "@/features/dashboard/components/detail-metric-table"
import { PayloadPanel } from "@/features/dashboard/components/payload-panel"
import type { ConsoleRequestDetail } from "@/features/dashboard/types"
import {
  extractReadableSseSegments,
  formatCount,
  formatDuration,
  formatTime,
  getCostMetricRows,
} from "@/features/dashboard/utils"

function statusStyle(code: number | null): { bg: string; fg: string } {
  if (code == null) return { bg: "var(--muted)", fg: "var(--lrs-faint)" }
  if (code >= 500) return { bg: "var(--lrs-danger-bg)", fg: "var(--lrs-danger)" }
  if (code >= 400) return { bg: "var(--lrs-warn-bg)", fg: "var(--lrs-warn)" }
  return { bg: "var(--lrs-success-bg)", fg: "var(--lrs-success)" }
}

function MetricCell({
  label,
  value,
  highlight,
}: {
  label: string
  value: string
  highlight?: boolean
}) {
  return (
    <div className="bg-card px-3 py-2.5">
      <div className="text-[10px] text-muted-foreground">{label}</div>
      <div
        className="mt-0.5 font-mono text-sm font-semibold"
        style={highlight ? { color: "var(--primary)" } : undefined}
      >
        {value}
      </div>
    </div>
  )
}

function ReadonlyTextCard({
  title,
  description,
  value,
  emptyTitle,
  emptyDescription,
  code = false,
}: {
  title: string
  description: string
  value: string
  emptyTitle: string
  emptyDescription: string
  /** 内容是 JSON 时用 CodeMirror 渲染（语法高亮 + 折叠 + 自动换行），与 payload 原始模式一致。 */
  code?: boolean
}) {
  const hasContent = value.trim().length > 0 && value.trim() !== "{}"

  return (
    <Card size="sm">
      <CardHeader className="border-b border-border/60">
        <CardTitle>{title}</CardTitle>
        <CardDescription>{description}</CardDescription>
      </CardHeader>
      <CardContent className="pt-4">
        {hasContent ? (
          code ? (
            <JsonCodeViewer value={value} />
          ) : (
            <Textarea
              readOnly
              value={value}
              className="min-h-64 w-full resize-none overflow-auto whitespace-pre-wrap break-all bg-background font-mono text-[11px] leading-5"
            />
          )
        ) : (
          <Empty className="border-border/70">
            <EmptyHeader>
              <EmptyTitle>{emptyTitle}</EmptyTitle>
              <EmptyDescription>{emptyDescription}</EmptyDescription>
            </EmptyHeader>
          </Empty>
        )}
      </CardContent>
    </Card>
  )
}

// 复制按钮：与 PayloadPanel 的复制行为一致（成功/失败 toast）。
function useCopyText() {
  const { t } = useTranslation()
  return (text: string) =>
    copyText(text).then((ok) =>
      ok ? toast.success(t("common.copied")) : toast.error(t("common.copyFailed")),
    )
}

// 思考过程：默认折叠的纯文本卡片。推理内容（reasoning_content / thinking）通常
// 较长，折叠后正文「内容」优先呈现，需要时再展开查看推理细节。
function SseReasoningCard({ value }: { value: string }) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const copy = useCopyText()
  return (
    <Card size="sm">
      <CardHeader className="border-b border-border/60 pb-4">
        <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
          <div className="space-y-1">
            <div className="flex items-center gap-1.5">
              <button
                type="button"
                onClick={() => setOpen(!open)}
                aria-expanded={open}
                className="flex items-center gap-1 text-foreground transition-colors hover:text-primary"
              >
                {open ? (
                  <ChevronDown className="h-4 w-4 text-muted-foreground" />
                ) : (
                  <ChevronRight className="h-4 w-4 text-muted-foreground" />
                )}
                <CardTitle className="text-amber-700 dark:text-amber-400">
                  {t("detail.sseReasoning")}
                </CardTitle>
              </button>
            </div>
            <CardDescription>{t("detail.sseReasoningDesc")}</CardDescription>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <span className="text-[11px] text-muted-foreground">
              {t("detail.sseReasoningCount", { count: formatCount(value.length) })}
            </span>
            <Button variant="ghost" size="sm" onClick={() => copy(value)}>
              <Copy className="h-3.5 w-3.5" />
              <span className="hidden sm:inline">{t("common.copy")}</span>
            </Button>
          </div>
        </div>
      </CardHeader>
      {open && (
        <CardContent className="pt-4">
          <div className="max-h-[32rem] overflow-auto whitespace-pre-wrap break-words text-sm leading-relaxed text-foreground/90">
            {value}
          </div>
        </CardContent>
      )}
    </Card>
  )
}

// 内容：默认 Markdown 渲染（与「对话列表」里 assistant 文本气泡一致），可切换
// 查看 Markdown 源码（CodeMirror 高亮 + 自动换行），让最终回复正文可直接阅读。
function SseContentCard({ value }: { value: string }) {
  const { t } = useTranslation()
  const copy = useCopyText()
  const [mode, setMode] = useState<"rendered" | "source">("rendered")
  return (
    <Card size="sm">
      <CardHeader className="border-b border-border/60 pb-4">
        <div className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
          <div className="space-y-1">
            <CardTitle>{t("detail.sseContent")}</CardTitle>
            <CardDescription>{t("detail.sseContentDesc")}</CardDescription>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <div className="flex gap-1">
              <Button
                variant={mode === "rendered" ? "default" : "outline"}
                size="sm"
                onClick={() => setMode("rendered")}
              >
                {t("payload.mdRendered")}
              </Button>
              <Button
                variant={mode === "source" ? "default" : "outline"}
                size="sm"
                onClick={() => setMode("source")}
              >
                {t("payload.mdSource")}
              </Button>
            </div>
            <Button variant="ghost" size="sm" onClick={() => copy(value)}>
              <Copy className="h-3.5 w-3.5" />
              <span className="hidden sm:inline">{t("common.copy")}</span>
            </Button>
          </div>
        </div>
      </CardHeader>
      <CardContent className="pt-4">
        <div className="max-h-[40rem] overflow-auto">
          {mode === "rendered" ? (
            <MarkdownRenderer text={value} />
          ) : (
            <MarkdownCodeViewer value={value} />
          )}
        </div>
      </CardContent>
    </Card>
  )
}

// SSE 可读区：拆成「思考过程」+「内容」两段。两段都空时保留旧的「没有可拼接的
// 回复」空态，便于排查未提取到文本的流式响应。
function SseReadableSections({
  reasoning,
  content,
}: {
  reasoning: string
  content: string
}) {
  const { t } = useTranslation()
  const reasoningTrim = reasoning.trim()
  const contentTrim = content.trim()
  const hasReasoning = reasoningTrim.length > 0
  const hasContent = contentTrim.length > 0 && contentTrim !== "{}"

  if (!hasReasoning && !hasContent) {
    return (
      <Card size="sm">
        <CardHeader className="border-b border-border/60">
          <CardTitle>{t("detail.sseConcat")}</CardTitle>
          <CardDescription>{t("detail.sseConcatDesc")}</CardDescription>
        </CardHeader>
        <CardContent className="pt-4">
          <Empty className="border-border/70">
            <EmptyHeader>
              <EmptyTitle>{t("detail.noSseConcat")}</EmptyTitle>
              <EmptyDescription>{t("detail.noSseConcatDesc")}</EmptyDescription>
            </EmptyHeader>
          </Empty>
        </CardContent>
      </Card>
    )
  }

  return (
    <div className="space-y-3">
      {hasReasoning && <SseReasoningCard value={reasoningTrim} />}
      {hasContent && <SseContentCard value={contentTrim} />}
    </div>
  )
}

export function DetailView({
  detail,
  error,
}: {
  detail: ConsoleRequestDetail | null
  error: string
}) {
  const { t } = useTranslation()
  // 注意：所有 hook 必须在任何 early return 之前调用，否则 detail 由非空变空时
  // hook 数量变化会触发 "Rendered fewer hooks than expected"，整棵树卸载白屏。
  const isMobile = useIsMobile()

  // Empty state
  if (!detail) {
    return (
      <div className="flex h-full flex-col">
        <div className="flex flex-1 items-center justify-center p-8">
          <Empty className="border-0">
            <EmptyHeader>
              <EmptyTitle>{t("detail.emptyTitle")}</EmptyTitle>
              <EmptyDescription>
                {error || t("detail.emptyDesc")}
              </EmptyDescription>
            </EmptyHeader>
          </Empty>
        </div>
      </div>
    )
  }

  const record = detail.record
  const usage = record.response_usage ?? {}
  const timing = record.response_timing ?? {}
  const st = statusStyle(record.response_status)

  // Request meta rows for request tab
  const originalHeadersText = JSON.stringify(record.original_headers ?? {}, null, 2)
  const forwardHeadersText = JSON.stringify(record.forward_headers ?? {}, null, 2)
  const responseHeadersText = JSON.stringify(record.response_headers ?? {}, null, 2)
  const sseSegments = timing.has_streaming_content
    ? extractReadableSseSegments(record.response_payload)
    : { reasoning: "", content: "" }

  const inputTokens = usage.uncached_input_tokens ?? usage.input_tokens ?? usage.total_input_tokens ?? 0
  const outputTokens = usage.output_tokens ?? usage.total_output_tokens ?? 0
  const cacheReadTokens = record.upstream_type === "openai"
    ? Number(usage.cached_input_tokens ?? 0)
    : Number(usage.cache_read_input_tokens ?? 0)
  const cacheCreationTokens = record.upstream_type === "openai"
    ? 0
    : Number(usage.cache_creation_input_tokens ?? usage.total_cache_creation_tokens ?? 0)
  const costRows = getCostMetricRows(usage, record.request_model, record.upstream_type)

  // Shared blocks — rendered by both the desktop pinned layout and the mobile
  // single-scroll layout, so the two stay in sync without duplicating content.
  const headerBlock = (
    <div className="shrink-0 border-b border-border px-3 py-3 sm:px-6 sm:py-4">
      {/* Top: status + model + channel */}
      <div className="flex min-w-0 items-center gap-2 sm:gap-3">
        <span
          className="shrink-0 rounded-md px-2.5 py-1 font-mono text-xs font-bold"
          style={{ background: st.bg, color: st.fg }}
        >
          {record.response_status ?? "--"}
        </span>
        <span className="min-w-0 truncate text-[15px] font-bold text-foreground">
          {record.request_model}
        </span>
        <span className="shrink-0 whitespace-nowrap text-xs text-muted-foreground">· {record.route_prefix}</span>
      </div>

      {/* request_id + 复制按钮：始终可见（含移动端），便于查看日志 id 排查 */}
      <div className="mt-2 flex items-center gap-1.5">
        <span className="shrink-0 text-[11px] text-muted-foreground/70">
          {t("detail.requestId")}
        </span>
        <code
          className="min-w-0 flex-1 truncate font-mono text-[11px] text-foreground/80"
          title={record.request_id}
        >
          {record.request_id}
        </code>
        <button
          type="button"
          title={t("detail.copyRequestId")}
          className="flex shrink-0 items-center gap-1 rounded-md px-1.5 py-1 text-[11px] font-medium text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
          onClick={() => {
            copyText(record.request_id).then((ok) =>
              ok ? toast.success(t("common.copied")) : toast.error(t("common.copyFailed")),
            )
          }}
        >
          <Copy className="h-3 w-3" />
          <span className="hidden sm:inline">{t("common.copy")}</span>
        </button>
      </div>
      {error ? (
        <Alert variant="destructive" className="mt-3">
          <AlertTitle>{t("detail.refreshFailed")}</AlertTitle>
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : null}

      {/* 4×2 metric grid — 首包/首Token/总耗时/生成 + 输入/输出/cache_read/cache_creation */}
      <div
        className="mt-3 grid grid-cols-2 gap-px overflow-hidden rounded-lg border border-border bg-border sm:mt-4 sm:grid-cols-4"
      >
        <MetricCell
          label={t("detail.firstChunk")}
          value={formatDuration(timing.first_chunk_latency_ms)}
        />
        <MetricCell
          label={t("detail.firstToken")}
          value={formatDuration(timing.first_token_latency_ms)}
          highlight
        />
        <MetricCell
          label={t("detail.duration")}
          value={formatDuration(timing.duration_ms)}
        />
        <MetricCell
          label={t("detail.generationDuration")}
          value={formatDuration(timing.generation_duration_ms)}
        />
        <MetricCell
          label={t("detail.inputTokens")}
          value={formatCount(inputTokens)}
        />
        <MetricCell
          label={t("detail.outputTokens")}
          value={formatCount(outputTokens)}
        />
        <MetricCell
          label="cache_read"
          value={formatCount(cacheReadTokens)}
        />
        <MetricCell
          label="cache_creation"
          value={formatCount(cacheCreationTokens)}
        />
      </div>
    </div>
  )

  const tabsListBlock = (
    // Tabs: 原始请求 / 转发请求 / 响应 / 成本 — Design: LRS Clear 风格五
    <TabsList
      variant="line"
      className="shrink-0 !h-auto w-full justify-start gap-0 overflow-x-auto border-b border-border bg-transparent px-3 py-0 sm:px-6"
    >
      <TabsTrigger
        value="request"
        className="mr-4 h-auto flex-none px-0.5 py-[11px] text-[13px] font-medium text-muted-foreground after:bottom-0 data-[state=active]:font-bold data-[state=active]:text-foreground sm:mr-6"
        style={{ '--tabs-line-color': 'var(--primary)', '--tabs-line-bottom': '0px' } as React.CSSProperties}
      >
        {t("detail.tabRequest")}
      </TabsTrigger>
      <TabsTrigger
        value="forward"
        className="mr-4 h-auto flex-none px-0.5 py-[11px] text-[13px] font-medium text-muted-foreground after:bottom-0 data-[state=active]:font-bold data-[state=active]:text-foreground sm:mr-6"
        style={{ '--tabs-line-color': 'var(--primary)', '--tabs-line-bottom': '0px' } as React.CSSProperties}
      >
        {t("detail.tabForward")}
      </TabsTrigger>
      <TabsTrigger
        value="response"
        className="mr-4 h-auto flex-none px-0.5 py-[11px] text-[13px] font-medium text-muted-foreground after:bottom-0 data-[state=active]:font-bold data-[state=active]:text-foreground sm:mr-6"
        style={{ '--tabs-line-color': 'var(--primary)', '--tabs-line-bottom': '0px' } as React.CSSProperties}
      >
        {t("detail.tabResponse")}
      </TabsTrigger>
      <TabsTrigger
        value="cost"
        className="mr-4 h-auto flex-none px-0.5 py-[11px] text-[13px] font-medium text-muted-foreground after:bottom-0 data-[state=active]:font-bold data-[state=active]:text-foreground sm:mr-6"
        style={{ '--tabs-line-color': 'var(--primary)', '--tabs-line-bottom': '0px' } as React.CSSProperties}
      >
        {t("detail.tabCost")}
      </TabsTrigger>
    </TabsList>
  )

  const tabPanelsBlock = (
    <div className="p-3 sm:p-4">
      {/* Request Tab */}
      <TabsContent value="request" className="mt-0">
        <div className="space-y-3">
          <PayloadPanel
            title={t("detail.originalPayload")}
            payload={record.original_payload}
            truncated={record.original_payload_truncated}
          />
          <ReadonlyTextCard
            title={t("detail.originalHeaders")}
            description={t("detail.originalHeadersDesc")}
            value={originalHeadersText}
            emptyTitle={t("detail.noOriginalHeaders")}
            emptyDescription={t("detail.noOriginalHeadersDesc")}
            code
          />
          {/* Meta info */}
          <Card size="sm">
            <CardHeader className="border-b border-border/60">
              <CardTitle>{t("detail.overviewTitle")}</CardTitle>
              <CardDescription>{t("detail.overviewDesc")}</CardDescription>
            </CardHeader>
            <CardContent className="pt-3">
              <div className="grid grid-cols-1 gap-x-4 gap-y-2 text-xs sm:grid-cols-2">
                <div className="text-muted-foreground">{t("detail.requestId")}</div>
                <div className="break-all font-mono text-foreground">{record.request_id}</div>
                <div className="text-muted-foreground">{t("detail.time")}</div>
                <div className="font-mono text-foreground">{formatTime(record.created_at)}</div>
                <div className="text-muted-foreground">{t("detail.path")}</div>
                <div className="break-all font-mono text-foreground">{record.path}</div>
                <div className="text-muted-foreground">{t("detail.keySource")}</div>
                <div className="text-foreground">{detail.client_label || "generic"}</div>
                <div className="text-muted-foreground">{t("detail.targetUrl")}</div>
                <div className="break-all font-mono text-foreground">{record.target_url}</div>
                <div className="text-muted-foreground">{t("detail.upstreamType")}</div>
                <div className="text-foreground">{record.upstream_type}</div>
                {record.failover_from ? (
                  <>
                    <div className="text-muted-foreground">{t("detail.failoverReason")}</div>
                    <div className="text-foreground">{record.failover_reason || record.failover_from}</div>
                  </>
                ) : null}
              </div>
            </CardContent>
          </Card>
        </div>
      </TabsContent>

      {/* Forward Tab */}
      <TabsContent value="forward" className="mt-0">
        <div className="space-y-3">
          <PayloadPanel
            title={t("detail.forwardedPayload")}
            payload={record.forwarded_payload}
            truncated={record.forwarded_payload_truncated}
          />
          <ReadonlyTextCard
            title={t("detail.forwardedHeaders")}
            description={t("detail.forwardedHeadersDesc")}
            value={forwardHeadersText}
            emptyTitle={t("detail.noForwardedHeaders")}
            emptyDescription={t("detail.noForwardedHeadersDesc")}
            code
          />
        </div>
      </TabsContent>

      {/* Response Tab */}
      <TabsContent value="response" className="mt-0">
        <div className="space-y-3">
          {timing.has_streaming_content ? (
            <SseReadableSections reasoning={sseSegments.reasoning} content={sseSegments.content} />
          ) : null}
          <PayloadPanel
            title={t("detail.responseBody")}
            payload={record.response_payload}
            truncated={record.response_payload_truncated}
          />
          <ReadonlyTextCard
            title={t("detail.responseHeaders")}
            description={t("detail.responseHeadersDesc")}
            value={responseHeadersText}
            emptyTitle={t("detail.noResponseHeaders")}
            emptyDescription={t("detail.noResponseHeadersDesc")}
            code
          />
        </div>
      </TabsContent>

      {/* Cost Tab */}
      <TabsContent value="cost" className="mt-0">
        <Card size="sm">
          <CardHeader className="border-b border-border/60">
            <CardTitle>{t("detail.costTitle")}</CardTitle>
            <CardDescription>{t("detail.costDesc")}</CardDescription>
          </CardHeader>
          <CardContent className="pt-3">
            <DetailMetricTable rows={costRows} />
          </CardContent>
        </Card>
      </TabsContent>
    </div>
  )

  const bottomBarBlock = (
    <div className="flex shrink-0 items-center gap-3 border-t border-border px-3 py-3 sm:gap-4 sm:px-6">
      <span className="hidden text-[11.5px] text-muted-foreground sm:inline">
        {t("detail.bottomHint")}
      </span>
      <div className="ml-auto flex items-center gap-4">
        <button
          type="button"
          className="shrink-0 rounded-md px-2 py-1.5 text-[11.5px] font-semibold text-primary transition-colors hover:bg-accent"
          onClick={() => {
            copyText(JSON.stringify(record, null, 2)).then((ok) =>
              ok ? toast.success(t("common.copied")) : toast.error(t("common.copyFailed")),
            )
          }}
        >
          {t("detail.copy")}
        </button>
      </div>
    </div>
  )

  // Mobile: one scroll region — header + tabs + panels + bottom bar scroll
  // together so the fixed metric header doesn't eat the viewport.
  if (isMobile) {
    return (
      <div className="flex h-full flex-col bg-card">
        <ScrollArea className="min-h-0 flex-1">
          {headerBlock}
          <Tabs defaultValue="request" className="flex flex-col">
            {tabsListBlock}
            {tabPanelsBlock}
          </Tabs>
          {bottomBarBlock}
        </ScrollArea>
      </div>
    )
  }

  // Desktop: pin header / tabs / bottom bar; only the panel content scrolls.
  return (
    <div className="flex h-full flex-col bg-card">
      {headerBlock}
      <Tabs defaultValue="request" className="flex min-h-0 flex-1 flex-col">
        {tabsListBlock}
        <ScrollArea className="min-h-0 flex-1">{tabPanelsBlock}</ScrollArea>
      </Tabs>
      {bottomBarBlock}
    </div>
  )
}
