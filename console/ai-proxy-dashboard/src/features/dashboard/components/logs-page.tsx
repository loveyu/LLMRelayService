import { useEffect, useMemo, useRef, useState } from "react"
import {
  Loader2,
  Radio,
  RotateCcw,
  Search,
} from "lucide-react"
import { useTranslation } from "react-i18next"

import { Button } from "@/components/ui/button"
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Combobox } from "@/components/ui/combobox"
import { Input } from "@/components/ui/input"
import { Pagination } from "@/components/ui/pagination"
import { Sheet, SheetContent } from "@/components/ui/sheet"
import { RequestLogTable } from "@/features/dashboard/components/request-log-table"
import { DetailView } from "@/features/dashboard/components/detail-view"
import { useDashboardData } from "@/features/dashboard/hooks/use-dashboard-data"
import { useIsMobile } from "@/hooks/use-is-mobile"
import { fetchRequestDetail } from "@/features/dashboard/api"
import type { RequestSortKey, SortDirection } from "@/features/dashboard/api"
import type { ConsoleRequestDetail } from "@/features/dashboard/types"

export type LogsTimeRange = "1h" | "6h" | "24h" | "7d" | "all"

export function LogsPage({
  onUnauthorized,
}: {
  onUnauthorized: () => void
}) {
  const {
    loading,
    refreshing,
    refreshDashboard,
    requests,
    total,
    limit,
    offset,
    filterOptions,
    sortBy,
    sortOrder,
  } = useDashboardData(onUnauthorized)

  const [searchQuery, setSearchQuery] = useState("")
  const [debouncedSearchQuery, setDebouncedSearchQuery] = useState("")
  const [routeFilter, setRouteFilter] = useState("")
  const [modelFilter, setModelFilter] = useState("")
  const [sourceTypeFilter, setSourceTypeFilter] = useState("")
  const [statusFilter, setStatusFilter] = useState("")
  const [cacheFilter, setCacheFilter] = useState("")
  const [timeRange, setTimeRange] = useState<LogsTimeRange>("1h")
  const [liveMode, setLiveMode] = useState(false)
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [detail, setDetail] = useState<ConsoleRequestDetail | null>(null)
  const [detailError, setDetailError] = useState("")
  const [detailLoading, setDetailLoading] = useState(false)
  const detailLoadRef = useRef(0)
  const { t } = useTranslation()
  const isMobile = useIsMobile()

  useEffect(() => {
    if (!selectedId) {
      setDetail(null)
      setDetailError("")
      setDetailLoading(false)
      return
    }
    const loadId = ++detailLoadRef.current
    setDetail(null)
    setDetailError("")
    setDetailLoading(true)
    void (async () => {
      try {
        const data = await fetchRequestDetail(selectedId)
        if (loadId !== detailLoadRef.current) return
        setDetail(data)
      } catch (err) {
        if (loadId !== detailLoadRef.current) return
        const message = err instanceof Error ? err.message : String(err)
        if (message === "unauthorized") {
          onUnauthorized()
          return
        }
        setDetailError(message)
      } finally {
        if (loadId === detailLoadRef.current) setDetailLoading(false)
      }
    })()
  }, [selectedId, onUnauthorized])

  // 防抖搜索 300ms
  useEffect(() => {
    const timer = window.setTimeout(() => {
      setDebouncedSearchQuery(searchQuery)
    }, 300)
    return () => window.clearTimeout(timer)
  }, [searchQuery])

  const filters = useMemo(() => ({
    search: debouncedSearchQuery || undefined,
    route: routeFilter || undefined,
    model: modelFilter || undefined,
    api_key_name: sourceTypeFilter || undefined,
    status: statusFilter || undefined,
    cache: cacheFilter || undefined,
  }), [debouncedSearchQuery, routeFilter, modelFilter, sourceTypeFilter, statusFilter, cacheFilter])

  const isFirstRenderRef = useRef(true)
  useEffect(() => {
    if (isFirstRenderRef.current) {
      isFirstRenderRef.current = false
      void refreshDashboard({ filters })
      return
    }
    void refreshDashboard({ filters, offset: 0 })
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filters])

  const filtersRef = useRef(filters)
  filtersRef.current = filters

  // Live mode: poll every 3s, limit 100, offset 0
  const liveModeRef = useRef(liveMode)
  liveModeRef.current = liveMode
  useEffect(() => {
    if (!liveMode) return
    void refreshDashboard({ limit: 100, offset: 0, filters: filtersRef.current })
    const id = window.setInterval(() => {
      if (liveModeRef.current) {
        void refreshDashboard({ silent: true, limit: 100, offset: 0, filters: filtersRef.current })
      }
    }, 3000)
    return () => window.clearInterval(id)
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [liveMode])

  const handlePageChange = (newOffset: number) => {
    void refreshDashboard({ offset: newOffset, filters })
  }

  const handleLimitChange = (newLimit: number) => {
    void refreshDashboard({ limit: newLimit, offset: 0, filters })
  }

  const handleSortChange = (newSortBy: RequestSortKey, newSortOrder: SortDirection) => {
    void refreshDashboard({ sortBy: newSortBy, sortOrder: newSortOrder, filters })
  }

  const routeOptions = useMemo(() => {
    return filterOptions.routes.map((value) => ({ value, label: value }))
  }, [filterOptions.routes])

  const sourceTypeOptions = useMemo(() => {
    return filterOptions.clients.map((client) => ({
      value: client.value,
      label: client.label,
    }))
  }, [filterOptions.clients])

  const hasActiveFilters = searchQuery || routeFilter || modelFilter || sourceTypeFilter || statusFilter || cacheFilter

  const timeRangeOptions: { value: LogsTimeRange; label: string }[] = [
    { value: "1h", label: t("logs.timeRange1h") },
    { value: "6h", label: t("logs.timeRange6h") },
    { value: "24h", label: t("logs.timeRange24h") },
    { value: "7d", label: t("logs.timeRange7d") },
    { value: "all", label: t("logs.timeRangeAll") },
  ]

  return (
    <div className="flex h-full flex-col gap-0">
      {/* Filter Bar — inline, no Card */}
      <div className="flex flex-wrap items-center gap-2 border-b border-border bg-card px-3 py-2 sm:gap-2.5 sm:px-6 sm:py-3">
        {/* Search */}
        <div className="relative w-full sm:w-auto sm:flex-1 sm:max-w-[280px]">
          <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground/60" />
          <Input
            value={searchQuery}
            onChange={(event) => setSearchQuery(event.target.value)}
            placeholder={t("logs.searchPlaceholder")}
            className="h-9 pl-9 pr-3 text-xs"
          />
        </div>

        {/* Status */}
        <Select value={statusFilter} onValueChange={(v) => { setStatusFilter(v === "all" ? "" : v) }}>
          <SelectTrigger className="h-9 w-auto min-w-[88px] text-xs">
            <SelectValue placeholder={t("logs.allStatus")} />
          </SelectTrigger>
          <SelectContent>
            <SelectGroup>
              <SelectItem value="all">{t("logs.allStatus")}</SelectItem>
              <SelectItem value="success">{t("logs.statusSuccess")}</SelectItem>
              <SelectItem value="error">{t("logs.statusError")}</SelectItem>
            </SelectGroup>
          </SelectContent>
        </Select>

        {/* Route */}
        <Combobox
          options={routeOptions}
          value={routeFilter}
          onChange={(v) => setRouteFilter(v ?? "")}
          placeholder={t("logs.routePlaceholder")}
          searchPlaceholder={t("logs.searchRoute")}
          className="h-9 w-[124px] bg-card text-xs font-normal"
        />

        {/* Source (API Key) */}
        <Combobox
          options={sourceTypeOptions}
          value={sourceTypeFilter}
          onChange={(v) => setSourceTypeFilter(v ?? "")}
          placeholder={t("logs.sourcePlaceholder")}
          searchPlaceholder={t("logs.searchSource")}
          className="h-9 w-[132px] bg-card text-xs font-normal"
        />

        {/* Time Range */}
        <Select value={timeRange} onValueChange={(v) => setTimeRange(v as LogsTimeRange)}>
          <SelectTrigger className="h-9 w-auto min-w-[96px] text-xs font-semibold border-primary/30 bg-accent text-accent-foreground">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectGroup>
              {timeRangeOptions.map((opt) => (
                <SelectItem key={opt.value} value={opt.value}>
                  {opt.label}
                </SelectItem>
              ))}
            </SelectGroup>
          </SelectContent>
        </Select>

        {/* Result count */}
        <span className="ml-auto shrink-0 text-xs text-muted-foreground">
          {t("logs.resultCount", { count: total })}
        </span>

        {/* Clear */}
        {hasActiveFilters ? (
          <Button
            variant="ghost"
            size="sm"
            className="h-9 px-2.5 text-xs text-muted-foreground"
            onClick={() => {
              setSearchQuery("")
              setRouteFilter("")
              setModelFilter("")
              setSourceTypeFilter("")
              setStatusFilter("")
              setCacheFilter("")
            }}
          >
            {t("common.clearFilters")}
          </Button>
        ) : null}

        {/* Live mode + Refresh */}
        <Button
          type="button"
          size="sm"
          variant={liveMode ? "default" : "outline"}
          onClick={() => setLiveMode((v) => !v)}
          className={liveMode ? "animate-pulse" : ""}
        >
          <Radio data-icon="inline-start" className="h-4 w-4" />
          {t("nav.live")}
        </Button>
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={refreshing || liveMode}
          onClick={() => {
            void refreshDashboard({ filters: filtersRef.current })
          }}
        >
          <RotateCcw
            data-icon="inline-start"
            className={`h-4 w-4 ${refreshing ? "animate-spin" : ""}`}
          />
          {t("common.refreshData")}
        </Button>
      </div>

      {/* Log list; details open in a responsive sheet on every viewport. */}
      <div className="flex min-h-0 flex-1 flex-col">
        {/* Left: compact log table */}
        <div className="flex h-full min-h-0 flex-col overflow-hidden">
          <RequestLogTable
            variant="compact"
            loading={loading}
            refreshing={refreshing}
            requests={requests}
            selectedId={selectedId}
            sortBy={sortBy}
            sortOrder={sortOrder}
            onSort={handleSortChange}
            onSelect={(requestId) => setSelectedId(requestId)}
            onApplyRouteFilter={setRouteFilter}
            onApplyModelFilter={setModelFilter}
            onApplySourceTypeFilter={setSourceTypeFilter}
          />

          {!liveMode && (
            <div className="shrink-0 border-t border-border px-3 py-2 sm:px-4 sm:py-3">
              <Pagination
                total={total}
                limit={limit}
                offset={offset}
                onPageChange={handlePageChange}
                onLimitChange={handleLimitChange}
              />
            </div>
          )}
        </div>

      </div>

      {/* Details slide up on mobile and in from the right on larger screens. */}
      <Sheet
        open={selectedId !== null}
        onOpenChange={(open) => {
          if (!open) setSelectedId(null)
        }}
      >
        <SheetContent
          side={isMobile ? "bottom" : "right"}
          className={isMobile
            ? "h-[88vh] max-h-[88vh] gap-0 p-0"
            : "h-full w-[min(860px,75vw)] max-w-none gap-0 p-0"}
        >
          {isMobile ? (
            <div className="flex shrink-0 justify-center pb-1 pt-2.5">
              <span className="h-1.5 w-10 rounded-full bg-border" />
            </div>
          ) : null}
          <div className="flex min-h-0 flex-1 flex-col overflow-hidden">
            {detailLoading ? (
              <div className="flex flex-1 flex-col items-center justify-center gap-2 text-muted-foreground">
                <Loader2 className="size-5 animate-spin" />
                <span className="text-xs">{t("detail.loadingDesc")}</span>
              </div>
            ) : (
              <DetailView detail={detail} error={detailError} />
            )}
          </div>
        </SheetContent>
      </Sheet>
    </div>
  )
}
