#!/usr/bin/env bash
# 开发模式启动/停止脚本（host 直跑，不用 Docker）：
#   ./run.sh start [port]   启动后端 (cargo run) + 前端 (vite dev server)
#   ./run.sh stop           停止
#   ./run.sh restart [port] 重启（stop 后 start）
#   ./run.sh status         查看运行状态
#   ./run.sh logs [be|fe]   查看后端/前端日志（默认全部）
# port 默认 12000；前端 dev server 默认 12001，/api 与 /v1 自动代理到后端。
# 都是高位端口，避免和机器上别的服务（3000/5173/8080 之类）抢。
# 可选环境变量（也可写在根目录 .env 里，run.sh 会自动加载）：
#   PORT                        后端端口（默认 12000；命令行第二个参数优先）
#   LITEROUTER_FRONTEND_PORT    前端端口（默认 12001）
#   LITEROUTER_DB               数据库路径（默认 data/literouter.db）
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOG_DIR="$ROOT/.run/logs"
PID_DIR="$ROOT/.run"
mkdir -p "$LOG_DIR" "$PID_DIR"

# 加载根目录 .env（如果有），让 cargo run / npm run dev 都能继承其中的变量。
if [ -f "$ROOT/.env" ]; then
    set -a
    # shellcheck disable=SC1091
    source "$ROOT/.env"
    set +a
fi

PORT="${2:-${PORT:-12000}}"
DB="${LITEROUTER_DB:-$ROOT/data/literouter.db}"
FRONTEND_PORT="${LITEROUTER_FRONTEND_PORT:-12001}"

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
    echo "端口: 后端 $PORT, 前端 $FRONTEND_PORT"
    start_one backend "$PID_DIR/backend.pid" "$ROOT/backend" \
        env PORT="$PORT" LITEROUTER_DB="$DB" cargo run
    start_one frontend "$PID_DIR/frontend.pid" "$ROOT/frontend" \
        env LITEROUTER_PORT="$PORT" LITEROUTER_FRONTEND_PORT="$FRONTEND_PORT" npm run dev
    echo
    echo "前端(带后台): http://localhost:$FRONTEND_PORT"
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

cmd_restart() {
    cmd_stop
    # Give the old processes a moment to release their ports before rebinding.
    sleep 1
    cmd_start
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
    start)   cmd_start ;;
    stop)    cmd_stop ;;
    restart) cmd_restart ;;
    status)  cmd_status ;;
    logs)    cmd_logs "${2:-all}" ;;
    *)
        echo "用法: $0 {start [port] | stop | restart [port] | status | logs [backend|frontend]}"
        exit 1
        ;;
esac
