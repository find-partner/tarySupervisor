# TraySupervisor 系统冒烟（Windows）—— 对应 docs/SYSTEM-TEST.md §4 脚本化用例 S-06 ~ S-07
#
# 用法：  powershell -ExecutionPolicy Bypass -File scripts\smoke-windows.ps1 [-AppPath target\debug\traysupervisor.exe]
# 前置：  cargo build -p traysupervisor（Windows MSVC 工具链）；GUI 会话（托盘需要）
# 说明：  用例 1 复现 SRS FR-3 验收场景（cmd → ping 孙进程链整树终止，spike 已验证的机制）；
#         用例 2 验证 KILL_ON_JOB_CLOSE 兜底（应用被硬杀时服务仍随 Job 清理）；
#         用例 3 验证配置热加载（FR-2.6）。
#         脚本会临时替换 %USERPROFILE%\.traysupervisor\config.ini（自动备份，finally 恢复）。
param(
    [string]$AppPath = "target\debug\traysupervisor.exe"
)

$ErrorActionPreference = "Stop"
$script:Pass = 0
$script:Fail = 0

function Pass($m) { Write-Host "  [PASS] $m"; $script:Pass++ }
function Fail($m) { Write-Host "  [FAIL] $m"; $script:Fail++ }

if (-not (Test-Path $AppPath)) {
    Write-Host "未找到应用二进制：$AppPath（先执行 cargo build -p traysupervisor）"
    exit 2
}

$cfgDir = Join-Path $env:USERPROFILE ".traysupervisor"
$cfg = Join-Path $cfgDir "config.ini"
$backup = "$cfg.smoke-bak"
$appLog = Join-Path $env:TEMP "traysv-smoke-app.log"
New-Item -ItemType Directory -Force -Path $cfgDir | Out-Null
$hadBackup = Test-Path $cfg
if ($hadBackup) { Copy-Item $cfg $backup -Force }

function Write-Config([string]$text) {
    # UTF-8 无 BOM（解析器已容忍 BOM，但保持干净）
    [IO.File]::WriteAllText($cfg, $text, [Text.UTF8Encoding]::new($false))
}

function PingCount { @(Get-Process ping -ErrorAction SilentlyContinue).Count }

try {
    Write-Host "== S-06 服务树整树终止 + 硬杀兜底（FR-3 / HLD D-5）"
    # 场景：cmd → ping（孙进程，ping -t 持续运行）；基线 = 已存在的 ping 进程数
    $baseline = PingCount
    Write-Config "[program:smoke-tree]`ncommand=cmd /c ping -t 127.0.0.1`nautostart=true`nstartsecs=0`nstopwaitsecs=5`n"
    $app = Start-Process -FilePath $AppPath -PassThru -RedirectStandardOutput $appLog -RedirectStandardError "$appLog.err"
    Start-Sleep -Seconds 6
    $during = PingCount
    if ($during -ge ($baseline + 1)) {
        Pass "autostart 服务树已拉起（ping.exe $baseline → $during）"
    } else {
        Fail "服务树未拉起（ping.exe $baseline → $during）"
    }

    # 应用被硬杀（TerminateProcess）：所有 Job 句柄随进程死亡关闭 → KILL_ON_JOB_CLOSE 兜底整树终止
    Stop-Process -Id $app.Id -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 5
    $after = PingCount
    if ($after -le $baseline) {
        Pass "应用硬杀后整树清理（KILL_ON_JOB_CLOSE 兜底）：ping.exe $after（基线 $baseline）"
    } else {
        Fail "应用硬杀后仍有残留 ping.exe：$after（基线 $baseline）"
        Get-Process ping -ErrorAction SilentlyContinue | Where-Object { $_.StartTime -gt (Get-Date).AddMinutes(-2) } | Stop-Process -Force -ErrorAction SilentlyContinue
    }

    Write-Host "== S-07 配置热加载（FR-2.6）"
    Write-Config "[traysupervisor]`nlog_lines=2000`nnotify_on_failure=true`n"
    $app = Start-Process -FilePath $AppPath -PassThru -RedirectStandardOutput $appLog -RedirectStandardError "$appLog.err"
    Start-Sleep -Seconds 3
    Add-Content -Path $cfg -Value "`n[program:smoke-hr]`ncommand=cmd /c echo hi`n"
    Start-Sleep -Seconds 3
    # env_logger 输出到 stderr（应用默认 info 级），断言需同时查 stdout 与 stderr 重定向文件
    $applied = Select-String -Path $appLog, "$appLog.err" -Pattern "配置热加载已应用" -Quiet -ErrorAction SilentlyContinue
    if ($applied) {
        Pass "有效变更被热加载应用"
    } else {
        Fail "未观察到热加载应用日志"
    }
    Write-Config "[program:broken]`ncommand=cmd /c echo x`nbogus_key=1`n"
    Start-Sleep -Seconds 3
    $rejected = Select-String -Path $appLog, "$appLog.err" -Pattern "配置热加载校验失败" -Quiet -ErrorAction SilentlyContinue
    if ($rejected) {
        Pass "非法配置被拒绝并沿用上一次有效配置"
    } else {
        Fail "非法配置未被正确拒绝"
    }
    Stop-Process -Id $app.Id -Force -ErrorAction SilentlyContinue

    Write-Host "== S-08 配置目录与文件存在（NFR-5 权限收紧为 Unix 专属，Windows 记录性检查）"
    if (Test-Path $cfg) { Pass "config.ini 存在：$cfg" } else { Fail "config.ini 不存在" }
} finally {
    Get-Process traysupervisor -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "*$AppPath*" } | Stop-Process -Force -ErrorAction SilentlyContinue
    if ($hadBackup) { Move-Item $backup $cfg -Force } else { Remove-Item $cfg -Force -ErrorAction SilentlyContinue }
    Get-ChildItem "$cfgDir\smoke-*" -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue
}

Write-Host ""
Write-Host "==== 冒烟结果：PASS=$($script:Pass) FAIL=$($script:Fail) ===="
if ($script:Fail -eq 0) { exit 0 } else { exit 1 }
