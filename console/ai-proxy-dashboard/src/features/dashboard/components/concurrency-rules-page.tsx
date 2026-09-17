import { useCallback, useEffect, useState } from "react"
import { Gauge, Plus, RefreshCw } from "lucide-react"
import { PageHeader } from "@/components/ui/page-header"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card"
import { Field, FieldGroup, FieldLabel } from "@/components/ui/field"
import { Input } from "@/components/ui/input"
import { toast } from "@/components/ui/toast"
import { createConcurrencyRule, fetchConcurrencyRules } from "@/features/dashboard/api"
import type { ConcurrencyRulesPayload } from "@/features/dashboard/types"

export function ConcurrencyRulesPage({ onUnauthorized }: { onUnauthorized: () => void }) {
  const [data, setData] = useState<ConcurrencyRulesPayload | null>(null)
  const [name, setName] = useState("")
  const [maxConcurrency, setMaxConcurrency] = useState("1")
  const load = useCallback(async () => {
    try { setData(await fetchConcurrencyRules()) }
    catch (error) { if (error instanceof Error && error.message === "unauthorized") onUnauthorized(); else toast.error(String(error)) }
  }, [onUnauthorized])
  useEffect(() => { void load(); const timer = window.setInterval(() => void load(), 3000); return () => window.clearInterval(timer) }, [load])
  async function create() {
    try { await createConcurrencyRule({ name: name.trim(), maxConcurrency: Number(maxConcurrency) }); setName(""); setMaxConcurrency("1"); await load(); toast.success("并发规则已创建") }
    catch (error) { toast.error(error instanceof Error ? error.message : String(error)) }
  }
  const runtime = new Map((data?.runtime ?? []).map((item) => [item.id, item]))
  return <div className="space-y-5">
    <PageHeader icon={Gauge} title="并发控制规则" description="规则配置保存在数据库；当前占用由 Rust Proxy 内存实时维护。" actions={<Button size="sm" variant="outline" onClick={() => void load()}><RefreshCw data-icon="inline-start" />刷新</Button>} />
    <Card><CardHeader><CardTitle>新建规则</CardTitle><CardDescription>多个渠道选择同一规则时，共享同一个上游并发上限。</CardDescription></CardHeader><CardContent><FieldGroup className="grid gap-4 sm:grid-cols-[1fr_12rem_auto] sm:items-end"><Field><FieldLabel>规则名称</FieldLabel><Input value={name} onChange={(event) => setName(event.target.value)} placeholder="例如：账号 A" /></Field><Field><FieldLabel>最大并发</FieldLabel><Input inputMode="numeric" value={maxConcurrency} onChange={(event) => setMaxConcurrency(event.target.value)} /></Field><Button type="button" size="sm" onClick={() => void create()}><Plus data-icon="inline-start" />新建规则</Button></FieldGroup></CardContent></Card>
    <Card><CardHeader><CardTitle>全局规则并发</CardTitle><CardDescription>每 3 秒刷新一次规则维度的当前活跃上游请求数。</CardDescription></CardHeader><CardContent>{data?.rules.length ? <div className="divide-y border"><div className="grid grid-cols-[1fr_7rem_7rem_7rem] gap-3 px-3 py-2 text-xs font-medium text-muted-foreground"><span>规则</span><span>当前并发</span><span>上限</span><span>绑定渠道</span></div>{data.rules.map((rule) => { const current = runtime.get(rule.id); return <div key={rule.id} className="grid grid-cols-[1fr_7rem_7rem_7rem] gap-3 px-3 py-3 font-mono text-xs"><span>{rule.name}</span><span>{current?.activeRequests ?? 0}</span><span>{rule.maxConcurrency}</span><span>{rule.providerCount}</span></div> })}</div> : <p className="text-sm text-muted-foreground">尚未创建并发规则。</p>}</CardContent></Card>
    <Card><CardHeader><CardTitle>渠道与模型实时并发</CardTitle><CardDescription>显示所有当前存在在途请求的渠道（包括未绑定并发规则的渠道），并在其下方按模型拆分。</CardDescription></CardHeader><CardContent>{data?.channels.length ? <div className="space-y-3">{data.channels.map((channel) => <div key={channel.channel} className="border"><div className="flex items-center justify-between px-3 py-2 font-mono text-xs"><span>{channel.channel}</span><span>当前并发 {channel.activeRequests}</span></div><div className="divide-y border-t">{channel.models.map((model) => <div key={model.model} className="flex items-center justify-between px-5 py-2 font-mono text-xs text-muted-foreground"><span>{model.model}</span><span>{model.activeRequests}</span></div>)}</div></div>)}</div> : <p className="text-sm text-muted-foreground">当前没有在途上游请求。</p>}</CardContent></Card>
  </div>
}
