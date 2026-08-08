#!/usr/bin/env bash
# scripts/dev-tmux.sh
# 通过 tmux 编排 LRS dev 环境（vite 前端 + 后端 server；后端会 spawn rust-proxy 子进程）。
#
# 设计参考 astream-v3/.local/tmux.sh:
#   - 幂等: 会话/窗口已存在则复用, 不重复创建。
#   - 端口感知: 端口在监听就复用（本窗口或外部进程）, 只补齐未启动的服务; 不强杀外部进程。
#   - 进程感知: 窗口在但进程已退出（回到 shell）则自动重启; 窗口有非 shell 进程则告警不打断。
#   - 防前缀冲突: 拒绝与现有 tmux 会话同名前缀, 避免 send-keys / stop 打到错误会话。
#   - restart: 杀会话 → 轮询端口释放 → 清 IPC socket → 重建, 避免 bind 冲突。
#
# dev 进程可随意关闭/重启 —— 生产是独立容器（端口 3000/3301）, 与 dev（8300/3311/5180）互不冲突。
# 重启 dev 一律走本脚本, 不要手动 `bun run dev:server` / 单独 kill rust-proxy:
# 手动操作容易留孤儿 rust-proxy + stale IPC socket（/tmp/lrs-ipc.sock）, 下次 rust-proxy
# bind 到 stale listener、bridge 连不上, test 端点报「Rust 配置尚未同步」。
#
# 用法:
#   scripts/dev-tmux.sh            up — 创建/补齐会话（默认, 后台 detached）
#   scripts/dev-tmux.sh attach     创建/补齐后挂接
#   scripts/dev-tmux.sh restart    杀会话 → 等端口释放 → 重建（改了 .env 后用）
#   scripts/dev-tmux.sh stop       杀会话
#   scripts/dev-tmux.sh status     会话 + 各端口状态
#   tmux attach -t lrs-dev         挂进去看日志（Ctrl-b d 脱离, 不停进程）

set -euo pipefail
REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

SESSION="${LRS_TMUX_SESSION:-lrs-dev}"
VITE_PORT="${VITE_PORT:-5180}"
BACKEND_ORIGIN="${LRS_BACKEND_ORIGIN:-http://127.0.0.1:8300}"
IPC_SOCKET="${LRS_IPC_SOCKET:-/tmp/lrs-ipc.sock}"

# name | cwd | 探活端口 | 启动命令(剩余字段, 含空格)
SERVICES=(
  "backend|$REPO_DIR|8300|bun run dev:server"
  "vite|$REPO_DIR/console/ai-proxy-dashboard|${VITE_PORT}|LRS_BACKEND_ORIGIN=${BACKEND_ORIGIN} bun run dev -- --port ${VITE_PORT} --host 0.0.0.0"
)
# restart 后要等释放的端口（backend 8300 + rust-proxy 3311 + vite）—— 确保子进程也退出
RELEASE_PORTS=(8300 3311 "${VITE_PORT}")

port_open() { ss -ltn "sport = :$1" 2>/dev/null | tail -n +2 | grep -q .; }
session_exists() { tmux ls 2>/dev/null | awk -F: -v s="$1" '$1==s{f=1} END{exit !f}'; }
prefix_collisions() { tmux ls 2>/dev/null | awk -F: -v s="$1" '$1!=s && index($1,s)==1 {print $1}' || true; }

action="${1:-up}"
case "$action" in
  stop)
    coll="$(prefix_collisions "$SESSION")"
    if [ -n "$coll" ]; then echo "拒绝: '$SESSION' 是其它会话前缀 ($coll), 换名" >&2; exit 1; fi
    if session_exists "$SESSION"; then
      tmux kill-session -t "$SESSION"
      rm -f "$IPC_SOCKET"
      echo "stopped '$SESSION'（含 backend / rust-proxy / vite）"
    else
      echo "no session '$SESSION'"
    fi
    exit 0 ;;
  up|"") ;;
  attach|a) ;;
  restart) ;;
  status)
    if session_exists "$SESSION"; then
      echo "session '$SESSION' 运行中: tmux attach -t $SESSION"
      tmux list-windows -t "$SESSION" -F '  #{window_name}: #{pane_current_command}' 2>/dev/null
    else
      echo "session '$SESSION' 未运行（启动: $0）"
    fi
    for e in "${SERVICES[@]}"; do
      IFS='|' read -r name _ port _ <<< "$e"
      if port_open "$port"; then echo "  :${port} ${name} 监听中"; else echo "  :${port} ${name} 未监听"; fi
    done
    exit 0 ;;
  *)
    cat >&2 <<EOF
用法: $0 [up|attach|restart|stop|status]
  up       创建/补齐 $SESSION 会话（默认, 后台）
  attach   创建/补齐后挂接
  restart  杀会话 → 等端口释放 → 重建
  stop     杀会话
  status   会话 + 端口状态
EOF
    exit 1 ;;
esac

# 防前缀冲突（up/attach/restart 通用）
coll="$(prefix_collisions "$SESSION")"
if [ -n "$coll" ]; then
  echo "拒绝: 会话名 '$SESSION' 是已有会话前缀 ($coll), 换名" >&2
  exit 1
fi

# restart: 杀会话 → 轮询端口释放（含 rust-proxy 3311）→ 清 IPC socket → 落入 up 重建。
# 直接 kill-session 后立刻重建, 旧进程可能尚未释放端口, 会被 up 的端口感知分支误判为「在跑」。
if [ "$action" = restart ]; then
  if session_exists "$SESSION"; then
    tmux kill-session -t "$SESSION"
    echo "restart: stopped '$SESSION'"
  else
    echo "restart: 无既有会话 '$SESSION', 直接 up"
  fi
  for port in "${RELEASE_PORTS[@]}"; do
    w=0
    while port_open "$port" && [ "$w" -lt 60 ]; do sleep 0.2; w=$((w + 1)); done
    if port_open "$port"; then
      echo "restart: 警告 — :${port} 未在 ~12s 内释放, 重建可能冲突（或为外部进程占用, 将复用）" >&2
    fi
  done
  rm -f "$IPC_SOCKET"
fi

# 1) 建会话（占位窗口, 最后删）
if ! session_exists "$SESSION"; then
  tmux new-session -d -s "$SESSION" -n shell -c "$REPO_DIR"
  tmux set-option -t "$SESSION" renumber-windows on >/dev/null
fi

# 2) 每服务一个窗口, 端口/进程感知补齐或重启
echo "session '$SESSION':"
for entry in "${SERVICES[@]}"; do
  IFS='|' read -r name cwd port cmd <<< "$entry"

  newly=0
  if ! tmux list-windows -t "$SESSION" -F '#{window_name}' 2>/dev/null | grep -qx "$name"; then
    tmux new-window -t "$SESSION:" -n "$name" -c "$cwd"
    newly=1
  fi

  # 端口在监听 = 服务在跑（本窗口或外部进程）, 不打扰。
  if port_open "$port"; then
    if [ "$newly" -eq 1 ]; then
      tmux send-keys -t "$SESSION:$name" "echo '[external] :${port} 已被占用, ${name} 复用外部进程, 未在此窗口启动'" C-m
      echo "  [ext  ] $name — :${port} 已监听（复用外部进程）"
    else
      echo "  [skip ] $name — :${port} 已监听（在跑）"
    fi
    continue
  fi

  # 端口未监听: 仅当窗口已回到 shell（进程已退出）才重启, 否则告警不打断。
  cur="$(tmux display-message -p -t "$SESSION:$name" '#{pane_current_command}' 2>/dev/null || true)"
  case "$cur" in
    zsh|bash|sh|dash|fish)
      tmux send-keys -t "$SESSION:$name" "$cmd" C-m
      if [ "$newly" -eq 1 ]; then
        echo "  [start] $name — :${port}"
      else
        echo "  [restart] $name — 进程已退出, 重启（:${port}）"
      fi
      ;;
    *)
      echo "  [warn ] $name — :${port} 未监听, 但窗口进程为 '${cur}', 跳过自动重启（避免打断）"
      ;;
  esac
done

# 3) 删占位窗口
tmux kill-window -t "$SESSION:shell" 2>/dev/null || true

echo
echo "ready.  tmux attach -t $SESSION   |   stop: $0 stop   |   restart: $0 restart"
case "$action" in
  attach|a) exec tmux attach -t "$SESSION" ;;
esac
