import { useCallback, useEffect, useState } from "react"
import { AlertTriangle, CheckCircle2, Cpu, RefreshCw, RotateCw, XCircle } from "lucide-react"

import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import { Button } from "@/components/ui/button"
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"
import { Skeleton } from "@/components/ui/skeleton"
import { PageHeader } from "@/components/ui/page-header"
import {
  fetchRustProxyStatus,
  restartRustProxy,
} from "@/features/dashboard/api"
import type { RustProxyStatus } from "@/features/dashboard/types"

function formatUptime(ms: number): string {
  if (ms <= 0) return "—"
  const s = Math.floor(ms / 1000)
  if (s < 60) return `${s}s`
  const m = Math.floor(s / 60)
  if (m < 60) return `${m}m ${s % 60}s`
  const h = Math.floor(m / 60)
  if (h < 24) return `${h}h ${m % 60}m`
  const d = Math.floor(h / 24)
  return `${d}d ${h % 24}h ${m % 60}m`
}

function formatDate(ts: number): string {
  if (!ts) return "—"
  return new Date(ts).toLocaleString()
}

export function RustProxyPage({
  onUnauthorized,
}: {
  onUnauthorized: () => void
}) {
  const [status, setStatus] = useState<RustProxyStatus | null>(null)
  const [error, setError] = useState("")
  const [loading, setLoading] = useState(true)
  const [restarting, setRestarting] = useState(false)

  const load = useCallback(async () => {
    try {
      const data = await fetchRustProxyStatus()
      setStatus(data)
      setError("")
    } catch (err) {
      if (err instanceof Error && err.message === "unauthorized") {
        onUnauthorized()
        return
      }
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
    }
  }, [onUnauthorized])

  useEffect(() => {
    void load()
  }, [load])

  useEffect(() => {
    const id = setInterval(() => void load(), 5000)
    return () => clearInterval(id)
  }, [load])

  const handleRestart = useCallback(async () => {
    setRestarting(true)
    try {
      await restartRustProxy()
      await new Promise((r) => setTimeout(r, 2000))
      await load()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setRestarting(false)
    }
  }, [load])

  if (loading) {
    return <RustProxyPageSkeleton />
  }

  return (
    <div className="flex flex-col gap-6">
      <PageHeader
        icon={Cpu}
        title="Rust 代理引擎"
        description="Rust proxy 进程运行状态与健康检查"
        actions={
          <>
            <Button type="button" size="sm" variant="outline" onClick={load}>
              <RefreshCw />
              刷新
            </Button>
            <Button
              type="button"
              size="sm"
              onClick={handleRestart}
              disabled={restarting}
            >
              <RotateCw className={restarting ? "animate-spin" : ""} />
              {restarting ? "重启中..." : "重启代理"}
            </Button>
          </>
        }
      />

      {error ? (
        <Alert variant="destructive">
          <AlertTitle>加载失败</AlertTitle>
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : null}

      {status ? (
        <>
          <Card>
            <CardHeader className="pb-3">
              <CardTitle className="flex items-center gap-2 text-sm">
                {status.running ? (
                  <CheckCircle2 className="h-4 w-4 text-lrs-success" />
                ) : (
                  <XCircle className="h-4 w-4 text-destructive" />
                )}
                进程状态
              </CardTitle>
            </CardHeader>
            <CardContent>
              <div className="grid grid-cols-2 gap-x-8 gap-y-4 sm:grid-cols-4">
                <Stat label="状态" value={status.running ? "运行中" : "已停止"} valueMono />
                <Stat label="PID" value={status.pid != null ? String(status.pid) : "—"} valueMono />
                <Stat label="已运行" value={formatUptime(status.uptimeMs)} valueMono />
                <Stat label="重启次数" value={String(status.restartCount)} valueMono />
                <Stat label="监听地址" value={`${status.host}:${status.port}`} valueMono />
                <Stat label="启动时间" value={formatDate(status.startedAt)} valueMono />
                <Stat
                  label="最大重启"
                  value={`${status.restartCount >= 5 ? "已用尽" : `${5 - status.restartCount} 次剩余`}`}
                  valueMono
                />
              </div>
              <div className="mt-4 flex flex-col gap-1">
                <span className="text-[11px] uppercase tracking-wider text-muted-foreground">可执行文件路径</span>
                <span className="break-all font-mono text-xs text-muted-foreground">
                  {status.realpath ?? status.bin}
                </span>
                {status.realpath && status.realpath !== status.bin ? (
                  <span className="break-all font-mono text-[10px] text-muted-foreground/60">
                    bin: {status.bin}
                  </span>
                ) : null}
              </div>
            </CardContent>
          </Card>

          <Card>
            <CardHeader className="pb-3">
              <CardTitle className="flex items-center gap-2 text-sm">
                {status.health.ok ? (
                  <CheckCircle2 className="h-4 w-4 text-lrs-success" />
                ) : (
                  <AlertTriangle className="h-4 w-4 text-destructive" />
                )}
                健康检查
              </CardTitle>
            </CardHeader>
            <CardContent>
              <div className="grid grid-cols-1 gap-4">
                <div className="flex items-center gap-3 rounded-[10px] border bg-muted/30 px-4 py-3">
                  <div className="flex items-center gap-2">
                    <span
                      className="h-2.5 w-2.5 rounded-full"
                      style={{ background: status.health.ok ? "var(--lrs-success)" : "var(--lrs-danger)" }}
                    />
                    <span className="text-sm font-semibold">
                      {status.health.ok ? "健康" : "异常"}
                    </span>
                  </div>
                  <span className="text-xs text-muted-foreground">
                    最近检查: {formatDate(status.health.at)}
                  </span>
                  {status.health.error ? (
                    <span className="text-xs text-destructive">{status.health.error}</span>
                  ) : null}
                </div>
              </div>
            </CardContent>
          </Card>

          {status.running ? null : (
            <Card className="border-destructive/40 bg-destructive/5">
              <CardHeader className="pb-3">
                <CardTitle className="flex items-center gap-2 text-sm text-destructive">
                  <AlertTriangle className="h-4 w-4" />
                  代理未运行
                </CardTitle>
              </CardHeader>
              <CardContent>
                <p className="text-sm text-muted-foreground">
                  Rust 代理进程已停止。TS 将在 {status.restartCount >= 5 ? "5 次尝试后停止自动重启" : "最多 5 次"} 自动重启该进程。
                  {status.restartCount < 5 ? " 也可以点击上方「重启代理」手动重启。" : ""}
                </p>
              </CardContent>
            </Card>
          )}
        </>
      ) : null}
    </div>
  )
}

function Stat({
  label,
  value,
  valueMono,
}: {
  label: string
  value: string
  valueMono?: boolean
}) {
  return (
    <div className="flex flex-col gap-1">
      <span className="text-[11px] uppercase tracking-wider text-muted-foreground">{label}</span>
      <span className={valueMono ? "text-sm font-mono font-medium" : "text-sm font-medium"}>{value}</span>
    </div>
  )
}

function RustProxyPageSkeleton() {
  return (
    <div className="flex flex-col gap-6">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          <Skeleton className="h-5 w-5 rounded" />
          <div>
            <Skeleton className="h-6 w-36" />
            <Skeleton className="mt-1 h-4 w-56" />
          </div>
        </div>
        <div className="flex gap-2">
          <Skeleton className="h-8 w-16" />
          <Skeleton className="h-8 w-24" />
        </div>
      </div>
      <Card>
        <CardHeader className="pb-3">
          <Skeleton className="h-5 w-28" />
        </CardHeader>
        <CardContent>
          <div className="grid grid-cols-2 gap-x-8 gap-y-4 sm:grid-cols-4">
            {Array.from({ length: 7 }).map((_, i) => (
              <div key={i} className="flex flex-col gap-1">
                <Skeleton className="h-3 w-14" />
                <Skeleton className="h-5 w-20" />
              </div>
            ))}
          </div>
          <div className="mt-4 flex flex-col gap-1">
            <Skeleton className="h-3 w-24" />
            <Skeleton className="h-4 w-full" />
          </div>
        </CardContent>
      </Card>
      <Card>
        <CardHeader className="pb-3">
          <Skeleton className="h-5 w-28" />
        </CardHeader>
        <CardContent>
          <Skeleton className="h-10 w-full" />
        </CardContent>
      </Card>
    </div>
  )
}
