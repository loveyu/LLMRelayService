# LRS Rust 代理引擎 — 分阶段实现计划

## 架构图

```txt
Client ──► Rust Proxy (:3301) ──► 上游 LLM APIs
               │ IPC (Unix Socket)
               ▼
          TS 进程 (:3300) — 管理 API / Console UI / DB 写入
```

Rust 负责：路由匹配、认证、请求代理、body 变换、超时控制、故障转移
TS 负责：DB 写入（日志/配额）、配置 CRUD、Console UI、OpenAPI 管理接口

---

## Phase 1: Rust 项目骨架 + IPC 通道

### 目标
建立 Rust 项目结构，运行一个最小可工作的 Axum HTTP 服务器，并建立 TS ↔ Rust 的双向 IPC 通道。

### 任务
- [ ] `cargo init rust-proxy` 在项目根目录创建 Rust 项目
- [ ] 配置 `Cargo.toml`：axum, tokio (full), serde/serde_json, reqwest, tower-http, tracing
- [ ] Axum server 启动在 `127.0.0.1:3301`，提供 `/health` 端点
- [ ] 实现 Unix Domain Socket IPC 通道（`/tmp/lrs-ipc.sock`）
  - TS → Rust: 发送配置同步消息 (`sync_config`, `reload_config`)
  - Rust → TS: 发送日志消息 (`request_log`, `response_log`)
- [ ] TS 端：实现 `src/rust-bridge.ts` — IPC 客户端，启动时同步配置，接收日志
- [ ] 修改 `scripts/dev.ts` 添加 rust 进程的启动

### 验收
- `bun run dev` 同时启动 TS 和 Rust 进程
- `curl http://127.0.0.1:3301/health` 返回 200
- TS 启动后 Rust 进程收到配置同步消息并打印日志

---

## Phase 2: 核心代理转发

### 目标
Rust 接收客户端请求，根据配置路由到上游 LLM 提供商，返回响应。

### 任务
- [ ] 定义 Rust 端的配置数据结构（`RouteConfig`, `AliasConfig`, `UpstreamType`）
- [ ] 实现配置反序列化（从 TS 发送的 JSON）
- [ ] 实现路由匹配逻辑
  - `/providers/{channel}/...` 显式路由
  - 模型名称匹配路由（含别名解析）
  - 优先级排序
- [ ] 实现 HTTP 代理：接收请求 → 构建上游 URL → reqwest 转发
- [ ] Header 处理：去除 hop-by-hop headers，注入 route auth
- [ ] First-byte 超时控制（可配置）
- [ ] 流式响应直通（SSE，不做 body 变换）
- [ ] 修改 TS 端，使所有 proxy 请求转发到 Rust（或路由 `/v1/*` 到 Rust）

### 验收
- 通过 Rust 代理发送 OpenAI Chat Completion 请求，收到正确的上游响应
- 通过 Rust 代理发送 Anthropic Message 请求，收到正确的流式响应
- 显式路由 `/providers/{channel}/...` 工作正常
- 模型名称路由工作正常
- `/v1/models` 返回正确的模型列表

---

## Phase 3: 认证 & 故障转移

### 目标
实现 API key 认证和请求级别的故障转移。

### 任务
- [ ] 实现本地 API key 哈希查找
  - TS 同步所有非吊销 key 的 `(keyHash, keyInfo)` 到 Rust
  - Rust 对每个请求做 SHA-256 + 本地 HashMap 查找
- [ ] 实现 Gateway admin key 直通（匹配 `GATEWAY_API_KEY`）
- [ ] 实现故障转移循环
  - 同路由重试（`retryAttempts`）
  - fallback 到其他路由（`same_model` / `any_model` / `custom_model_fallbacks`）
- [ ] 实现 timeout/network_error/status_code 的故障触发检测
- [ ] 配额检查（从同步的 key info 判断 `quota_exhausted`）

### 验收
- 使用 managed API key 请求，认证通过
- 使用错误的 key，返回 401
- 上游超时后自动重试（同路由）
- 上游持续失败后 fallback 到备用路由
- `quota_exhausted` 的 key 返回 429

---

## Phase 4: Body 变换

### 目标
实现请求/响应 body 的必要变换。

### 任务
- [ ] Anthropic: system prompt 注入 (`injectRouteSystemIntoSystem`)
- [ ] Anthropic: claudeCodeCompat（system → first user message）
- [ ] 响应 model 名重写（alias 隐藏）
  - 流式响应中的正则替换 + 滑动窗口
  - 非流式响应中的 JSON 字段替换
- [ ] 响应 idle timeout（chunk 间超时）
- [ ] 按需实现 Anthropic thinking block 过滤

### 验收
- 配置了 system prompt 的路由，上游请求包含注入的 system
- claudeCodeCompat 路由正确注入 `<system_instructions>` tag
- 使用别名请求，响应中的 model 名被重写为别名
- 流式响应 chunk 间超时触发错误

---

## Phase 5: 日志回写

### 目标
Rust 将请求/响应日志通过 IPC 异步发送给 TS，TS 写入数据库。

### 任务
- [ ] Rust 端：定义 `RequestLog` 和 `ResponseLog` 结构
- [ ] Rust 端：实现异步日志发送（非阻塞，有队列缓冲）
- [ ] Rust 端：实现流式响应的 chunk 观察（tee/copy，类似 observePassthrough）
- [ ] Rust 端：非流式响应的 body clone/log（类似 observeDetachedResponseBody）
- [ ] TS 端：接收 IPC 日志消息，调用 `saveConsoleRequest` + `saveConsoleResponse`
- [ ] TS 端：接收配额扣减消息，调用 `syncApiKeyQuotaCharge`
- [ ] Backpressure 控制：日志队列满时丢弃而非阻塞代理

### 验收
- 请求完成后，console_requests 表有对应记录
- 响应完成后，response_status / tokens 等信息被更新
- API key 使用量正确扣减
- Rust 进程在 TS 进程挂掉时，日志不丢失（缓冲），代理不受影响

---

## Phase 6: 收尾

### 目标
生产就绪的双进程部署。

### 任务
- [ ] Graceful shutdown：SIGTERM 时等待飞行中的请求完成 + drain 日志队列
- [ ] Rust 进程健康检查（`/health` 包含 IPC 连通状态）
- [ ] 双进程生命周期管理（supervisord 或 Rust 管理 TS 子进程）
- [ ] Dockerfile 改造：多阶段构建含 Rust 二进制
- [ ] `docker-compose.yml` 更新
- [ ] 监控指标端点（请求数、延迟分布、错误率）
- [ ] 更新 changelog 和文档

### 验收
- `docker compose up` 同时启动 Rust + TS
- SIGTERM 后两个进程优雅退出
- 全部代理请求正常通过 Rust 转发
- `/health` 报告 IPC 连通状态
