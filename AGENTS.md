# AGENTS.md

## UI / Console Frontend Conventions

### shadcn/ui 风格要求
- Console 前端（`console/ai-proxy-dashboard`）的 UI 改动，默认必须遵循 **shadcn/ui** 风格与组件约定。
- 如果是图表相关需求，优先使用 **shadcn/ui chart primitives**（如 `ChartContainer`、`ChartTooltip`、`ChartLegend` 等）和其推荐结构；不要优先手搓一套视觉风格明显不一致的图表壳子。
- 页面中的 block / section 标题层级要统一：
  - 同级区块优先使用一致的 `CardHeader` / `CardTitle` / `CardDescription`
  - 不要在某些区块内部临时塞一套与其他 block 不一致的标题样式
- 图表**内部**可以根据数据表达需要做差异化设计，但图表**外部容器**（标题、描述、边距、分区方式）要与页面其他 card/block 对齐。
- 表格、图表、筛选区、概览卡片之间的文字层级、间距、容器边框风格要尽量统一，避免局部区域“单独一套设计语言”。

### Working Style
- 对这个仓库做代码修改时，若需求已经明确，默认直接改、验证、然后 push；不要停在口头方案阶段。
- 理解用户需求时，不要只按字面意思机械执行；要结合代码结构、已有实现模式、命名、仓库约定和上下文，优先推断用户在当前代码库中的真实意图。只有当关键语义存在多个合理解释且无法从代码库中消解时，再向用户确认。
- 做完前端改动后，至少运行：
  - `cd console/ai-proxy-dashboard && bun run typecheck`
  - `cd console/ai-proxy-dashboard && bun run build`
- 做完后端（`src/`）改动后，至少运行：
  - `bun run lint`（eslint src，抓 tsc 漏报的「类型层合法但运行时未 import」符号 —— 如 drizzle 的 `ne` 被 d.ts 全局化后 tsc 不报、但运行时 `ReferenceError`，曾导致 `/#/providers` 500）
  - `npx tsc --noEmit`（确认无新增类型错误；根 tsc 历史有几条 pre-existing 噪音，只关注自己改的文件）
- **push 前必须先启动本地 dev 环境验证无报错**：`scripts/dev-tmux.sh`（tmux 编排 vite + 后端 + rust-proxy），用 `scripts/dev-tmux.sh status` 与 `curl -s 127.0.0.1:3311/health`（看 `config_synced:true`）确认健康后再 push。详见下方「Dev 环境（tmux 编排）」。
- **数据库迁移：只用 drizzle-kit，禁用 inline migrations。**
  - 改 schema 后必须运行 `drizzle-kit generate` 生成迁移文件
  - **schema 改动和迁移文件必须在同一个 commit 中提交**，禁止只 push schema 不 push 迁移
  - 部署时自动执行 `drizzle-kit migrate`（Dockerfile CMD 已配置）
  - **禁止**在 `src/db/migrate.ts` 或 store 文件中写内联迁移
  - 如果 snapshot 损坏，从 git 历史恢复，不要删除后重新生成
- 对项目的功能/行为改动，默认**同步更新 changelog**；不要只改代码不记变更。
- **代码改动和对应的 changelog 更新必须在同一个 commit 中提交**，禁止拆分成多个 commit。
- 如果根目录 lockfile 因依赖变化被修改（例如 `bun.lock`），不要漏提、漏 push。
- **禁止将 API Key、token、密码等敏感凭证提交到仓库**。提交前检查变更中是否包含 `sk-`、`api_key`、`secret` 等敏感信息。
- **测试脚本（bench、compare 等一次性验证脚本）无需提交到仓库**，用完即删。

### Dev 环境（tmux 编排）

- 启动/重启 dev 一律用 `scripts/dev-tmux.sh`（**脚本入库、团队共用**）：在 tmux session `lrs-dev` 里编排 vite（:5180）+ 后端 server（:8300，自动 spawn rust-proxy :3311）。
  - `scripts/dev-tmux.sh` — 创建/补齐会话（默认 up，后台 detached）
  - `scripts/dev-tmux.sh attach` — 创建/补齐后挂接（`Ctrl-b d` 脱离，不停进程）
  - `scripts/dev-tmux.sh restart` — 杀会话 → 等端口释放 → 重建（改了 `.env` / 重新编译 rust-proxy 后用）
  - `scripts/dev-tmux.sh stop` / `status`
  - 幂等 + 端口感知：端口在监听就复用（含外部已启动的进程），只补齐没起的服务，不强杀外部进程；防 tmux 会话名前缀冲突。
- **禁止手动 `bun run dev:server` / 单独 `kill` rust-proxy**：手动启停会留孤儿 rust-proxy + stale IPC socket（`/tmp/lrs-ipc.sock`），下次 rust-proxy bind 到 stale listener、TS bridge 连不上，连通性测试报「Rust 配置尚未同步」。要重启 dev 走 `scripts/dev-tmux.sh restart`（kill 整个 tmux session → 进程组连带退出 → 干净冷启动）。
- **生产是独立容器**（端口 3000/3301），与 dev（8300/3311/5180）互不冲突，dev 进程可随意关闭/重启，不影响生产。

### Changelog 规范
- 变更日志统一存放在项目根目录 `changelog/` 文件夹下，**按日期一天一个文件**，文件名格式 `YYYY-MM-DD.md`（如 `changelog/2026-04-18.md`）。
- 完成任何需求后，必须在当天对应的 `changelog/YYYY-MM-DD.md` 中追加记录；若该文件不存在则新建。
- 文件格式：顶部 `# YYYY-MM-DD`，按 `### 新增` / `### 变更` / `### 修复` 分组用列表项描述变更，描述要简明且包含实现方式。
- **不再使用根目录 `CHANGELOG.md`**，历史内容已迁移至 `changelog/` 目录。

## Rust Proxy (`rust-proxy/`)

### 质量门禁
- 改动 Rust 代码后，**必须**在 push 前通过以下检查：
  - `cd rust-proxy && cargo fmt -- --check` — 格式检查（配置见 `rustfmt.toml`）
  - `cd rust-proxy && cargo clippy -- -D warnings` — 零 warning 门禁
  - `cd rust-proxy && cargo build` — 编译通过
- 如果 clippy 有无法自动修复的 warning，用 `#[expect(dead_code)]` 或 `#[allow(clippy::xxx)]` 显式标注，不要留 warning。
- 新增模块时在 `src/main.rs` 添加 `mod xxx;` 声明。

### 代码风格
- `rustfmt.toml`: `max_width = 100`, `tab_spaces = 4`, `edition = "2021"`
- 优先使用 `serde_json::Value` / `serde_json::json!()` 处理 JSON，与 TS 的 `JsonRecord` 对应
- 新模块用 `pub` 导出供外部调用，内部辅助函数保持 `fn`（不 pub）
- 错误处理：proxy handler 返回 `Result<Response, StatusCode>`；内部可失败逻辑用 `Result<_, Box<dyn Error>>`
