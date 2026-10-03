#!/usr/bin/env bash
# TraySupervisor 系统冒烟（macOS / Linux）—— 对应 docs/SYSTEM-TEST.md §4 脚本化用例 S-01 ~ S-05
#
# 用法：  scripts/smoke-unix.sh [应用二进制路径]      # 默认 target/debug/traysupervisor
# 前置：  cargo build -p traysupervisor；需在有 GUI 会话的本机运行（托盘创建需要）
# 注意：  脚本会临时替换 ~/.traysupervisor/config.ini（自动备份，退出时恢复），
#         并在本机拉起托盘应用进程，运行结束后全部清理。
set -u
cd "$(dirname "$0")/.."

APP="${1:-target/debug/traysupervisor}"
CFG_DIR="$HOME/.traysupervisor"
CFG="$CFG_DIR/config.ini"
APPLOG="$(mktemp /tmp/traysv-smoke-app.XXXXXX.log)"
PASS=0; FAIL=0
SLEEP_PAT='/bin/sleep 60'

ok()  { echo "  [PASS] $1"; PASS=$((PASS+1)); }
bad() { echo "  [FAIL] $1"; FAIL=$((FAIL+1)); }

[ -x "$APP" ] || { echo "未找到应用二进制：${APP}（先执行 cargo build -p traysupervisor）"; exit 2; }
command -v pgrep >/dev/null || { echo "需要 pgrep"; exit 2; }

mkdir -p "$CFG_DIR"

# 单实例锁（FR-1.9）：先结束同一路径的残留实例，避免后续启动被单实例机制拒绝
for pid in $(pgrep -f "$APP" 2>/dev/null); do
  if [ "$pid" != "$$" ]; then
    echo "  [准备] 结束残留应用实例 PID $pid"
    kill -9 "$pid" 2>/dev/null
    sleep 1
  fi
done

BACKUP="$(mktemp)"
[ -f "$CFG" ] && cp "$CFG" "$BACKUP"
CURRENT_APP_PID=""
restore() {
  [ -n "$CURRENT_APP_PID" ] && kill -TERM "$CURRENT_APP_PID" 2>/dev/null
  if [ -s "$BACKUP" ]; then cp "$BACKUP" "$CFG"; else rm -f "$CFG"; fi
  rm -f "$CFG_DIR"/smoke-*.log "$CFG_DIR"/smoke-*.log.* "$APPLOG"
}
trap restore EXIT

# 启动应用（$1=RUST_LOG 级别），stdout/stderr 进 ${APPLOG}，回显 PID
launch_app() {
  RUST_LOG="${1:-info}" "$APP" > "$APPLOG" 2>&1 &
  CURRENT_APP_PID=$!
  echo "$CURRENT_APP_PID"
}

echo "== S-01 autostart 拉起 + 信号退出清理（FR-2.1 / FR-3.7）"
cat > "$CFG" <<'EOF'
[program:smoke-persist]
command=/bin/sleep 60
autostart=true
startsecs=0
EOF
APP_PID=$(launch_app info)
sleep 4
CHILD=$(pgrep -f "$SLEEP_PAT" | head -1)
if [ -n "$CHILD" ] && [ "$(ps -o ppid= -p "$CHILD" 2>/dev/null | tr -d ' ')" = "$APP_PID" ]; then
  ok "autostart 服务已由应用拉起（sleep PID ${CHILD}，父进程 = 应用）"
else
  bad "autostart 服务未拉起或父子关系不符（CHILD=$CHILD APP_PID=${APP_PID}）"
fi
kill -TERM "$APP_PID" 2>/dev/null; wait "$APP_PID" 2>/dev/null || true
sleep 3
if ! pgrep -f "$SLEEP_PAT" >/dev/null 2>&1; then
  ok "应用退出（SIGTERM）后托管进程已清理（FR-3.7）"
else
  bad "应用退出后仍有残留进程（FR-3.7）"; pkill -f "$SLEEP_PAT" 2>/dev/null
fi

echo "== S-02 日志轮转：100KB × 保留 3 份（FR-4.3，SRS 正式版为 10MB×3 灌 50MB，见 ST-09）"
cat > "$CFG" <<EOF
[program:smoke-rotate]
command=/usr/bin/seq 1 200000
autostart=true
startsecs=0
stdout_logfile=$CFG_DIR/smoke-rotate.log
stdout_logfile_maxbytes=100000
stdout_logfile_backups=3
EOF
APP_PID=$(launch_app info)
sleep 5
kill -TERM "$APP_PID" 2>/dev/null; wait "$APP_PID" 2>/dev/null || true
N_FILES=$(ls "$CFG_DIR"/smoke-rotate.log* 2>/dev/null | wc -l | tr -d ' ')
TOTAL_KB=$(du -sk "$CFG_DIR"/smoke-rotate.log* 2>/dev/null | awk '{s+=$1} END {print s+0}')
if [ "${N_FILES:-0}" -ge 2 ] && [ "${N_FILES:-0}" -le 4 ]; then
  ok "轮转发生且保留份数受限：共 $N_FILES 个文件（主 + .1 .2 …）"
else
  bad "轮转文件数异常：${N_FILES}（预期 2~4）"
fi
if [ "${TOTAL_KB:-0}" -le 500 ]; then
  ok "总占用受限：${TOTAL_KB}KB（≈ 主文件上限 × 4）"
else
  bad "总占用超预期：${TOTAL_KB}KB"
fi

echo "== S-03 配置热加载：added 应用 + 校验失败沿用（FR-2.6）"
printf '[traysupervisor]\nlog_lines=2000\nnotify_on_failure=true\n' > "$CFG"
APP_PID=$(launch_app info)
sleep 2
printf '\n[program:smoke-hr]\ncommand=/bin/echo hi\n' >> "$CFG"
sleep 2
if grep -q "配置热加载已应用" "$APPLOG"; then
  ok "有效变更被热加载应用"
else
  bad "未观察到热加载应用日志"
fi
printf '[program:broken]\ncommand=/bin/echo x\nbogus_key=1\n' > "$CFG"
sleep 2
if grep -q "配置热加载校验失败" "$APPLOG"; then
  ok "非法配置被拒绝并沿用上一次有效配置"
else
  bad "非法配置未被正确拒绝"
fi
kill -TERM "$APP_PID" 2>/dev/null; wait "$APP_PID" 2>/dev/null || true

echo "== S-04 应用创建的配置文件权限 0600（NFR-5）"
rm -f "$CFG"
APP_PID=$(launch_app info)
sleep 2
kill -TERM "$APP_PID" 2>/dev/null; wait "$APP_PID" 2>/dev/null || true
perm=$(stat -f '%Lp' "$CFG" 2>/dev/null || stat -c '%a' "$CFG" 2>/dev/null)
if [ "$perm" = "600" ]; then
  ok "应用创建的 config.ini 权限 = 600"
else
  bad "config.ini 权限 = ${perm}（预期 600，实际由应用 ensure_default_config 创建）"
fi

echo "== S-05 菜单重建防抖合并（FR-1.6，RUST_LOG=debug 计数重建）"
{
  echo '[traysupervisor]'
  echo 'log_lines=2000'
  for i in 1 2 3 4 5; do
    echo
    echo "[program:smoke-w$i]"
    echo 'command=/bin/sleep 1'
    echo 'autostart=true'
    echo 'autorestart=always'
    echo 'startsecs=0'
    echo 'startretries=10'
  done
} > "$CFG"
APP_PID=$(launch_app debug)
sleep 6
kill -TERM "$APP_PID" 2>/dev/null; wait "$APP_PID" 2>/dev/null || true
REBUILDS=$(grep -c "托盘菜单已重建" "$APPLOG" || true)
# 5 个快速退出的 always 服务 6 秒内产生 ≈50+ 状态事件；
# 300ms 防抖合并后重建应显著更少（≤20）
if [ "${REBUILDS:-99}" -gt 0 ] && [ "${REBUILDS:-99}" -le 20 ]; then
  ok "防抖合并生效：6s 内 50+ 状态事件 → $REBUILDS 次重建"
else
  bad "防抖合并异常：重建次数 = ${REBUILDS:-?}（预期 1~20）"
fi

echo
echo "==== 冒烟结果：PASS=$PASS FAIL=$FAIL ===="
[ "$FAIL" -eq 0 ]
