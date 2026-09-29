#!/usr/bin/env bash
# 开发模式启动/停止脚本（host 直跑，不用 Docker）：
#   ./run.sh start [port]   启动后端 (cargo run) + 前端 (vite dev server)
#   ./run.sh stop           停止
#   ./run.sh status         查看运行状态
#   ./run.sh logs [be|fe]   查看后端/前端日志（默认全部）
# port 默认 3000；前端 dev server 固定 5173，/api 与 /v1 自动代理到后端。
# 可选环境变量：ADMIN_PASSWORD（后台密码，默认 admin123）、
#               LITEROUTER_DB（数据库路径，默认 dev.db）。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOG_DIR="$ROOT/.run/logs"
PID_DIR="$ROOT/.run"
mkdir -p "$LOG_DIR" "$PID_DIR"

PORT="${2:-3000}"
DB="${LITEROUTER_DB:-$ROOT/data/literouter.db}"

is_running() { # $1 = pidfile
    [ -f "$1" ] && kill -0 "$(cat "$1")" 2>/dev/null
}

start_one() { # $1 = name, $2 = pidfile, rest = command (在对应目录执行)
    local name="$1" pidfile="$2" dir="$3"; shift 3
    if is_running "$pidfile"; then
        echo "$name 已在运行 (pid $(cat "$pidfile"))"
        return
    fi
    (
        cd "$dir"
        exec setsid "$@"
    ) > "$LOG_DIR/$name.log" 2>&1 &
    echo $! > "$pidfile"
    echo "$name 已启动 (pid $(cat "$pidfile"))，日志: .run/logs/$name.log"
}

cmd_start() {
    if is_running "$PID_DIR/backend.pid" || is_running "$PID_DIR/frontend.pid"; then
        echo "已有开发进程在运行，先执行 ./run.sh stop" >&2
        exit 1
    fi
    echo "端口: 后端 $PORT, 前端 5173"
    start_one backend "$PID_DIR/backend.pid" "$ROOT/backend" \
        env PORT="$PORT" LITEROUTER_DB="$DB" cargo run
    start_one frontend "$PID_DIR/frontend.pid" "$ROOT/frontend" \
        env LITEROUTER_PORT="$PORT" npm run dev
    echo
    echo "前端(带后台): http://localhost:5173"
    echo "后端 API:     http://localhost:$PORT/v1"
}

stop_one() { # $1 = name, $2 = pidfile —— 杀整个进程组（setsid 后按 pgid 杀）
    local name="$1" pidfile="$2"
    if is_running "$pidfile"; then
        local pid
        pid="$(cat "$pidfile")"
        kill -- -"$pid" 2>/dev/null || kill "$pid" 2>/dev/null || true
        echo "$name 已停止 (pid $pid)"
    fi
    rm -f "$pidfile"
}

cmd_stop() {
    stop_one frontend "$PID_DIR/frontend.pid"
    stop_one backend "$PID_DIR/backend.pid"
}

cmd_status() {
    for name in backend frontend; do
        if is_running "$PID_DIR/$name.pid"; then
            echo "$name: 运行中 (pid $(cat "$PID_DIR/$name.pid"))"
        else
            echo "$name: 未运行"
        fi
    done
}

cmd_logs() {
    local which="${1:-all}"
    if [ "$which" = all ]; then
        tail -n 50 -f "$LOG_DIR/backend.log" "$LOG_DIR/frontend.log"
    else
        tail -n 50 -f "$LOG_DIR/$which.log"
    fi
}

case "${1:-}" in
    start)  cmd_start ;;
    stop)   cmd_stop ;;
    status) cmd_status ;;
    logs)   cmd_logs "${2:-all}" ;;
    *)
        echo "用法: $0 {start [port] | stop | status | logs [backend|frontend]}"
        exit 1
        ;;
esac
