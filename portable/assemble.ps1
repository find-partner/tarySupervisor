# 组装 PortableApps.com Format 包目录，供 PortableApps.comLauncherGenerator.exe
# 与 PortableApps.comInstaller.exe 消费（用法见 release.yml 的 paf.exe 步骤）。
#
# 用法：pwsh portable/assemble.ps1 -Exe <TraySupervisor.exe> -Out <输出根目录>
# 产物：<Out>/TraySupervisorPortable/（目录结构与 PA.c Format 3.9 一致）
#
# 可在任意装了 pwsh 的平台上干跑验证（不依赖 Windows）。
param(
    [Parameter(Mandatory = $true)][string] $Exe,
    [Parameter(Mandatory = $true)][string] $Out
)
$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not (Test-Path $Exe)) { throw "exe 不存在: $Exe" }

# 版本号：tauri.conf.json 的 version（三段）→ PA.c 要求四段 PackageVersion
$tauriConf = Get-Content (Join-Path $repoRoot "src-tauri/tauri.conf.json") -Raw | ConvertFrom-Json
$version = [string] $tauriConf.version
if ($version -notmatch '^\d+\.\d+\.\d+$') { throw "tauri.conf.json version 非三段格式: $version" }
$packageVersion = "$version.0"

$pkg = Join-Path $Out "TraySupervisorPortable"
if (Test-Path $pkg) { Remove-Item -Recurse -Force $pkg }
foreach ($dir in @(
    "App/TraySupervisor",
    "App/AppInfo/Launcher",
    "Data",
    "Other/Source"
)) { New-Item -ItemType Directory -Force -Path (Join-Path $pkg $dir) | Out-Null }

Copy-Item $Exe (Join-Path $pkg "App/TraySupervisor/TraySupervisor.exe")

# appinfo.ini：模板注入版本号（字段值不含双引号，PA.c 规范要求）
$appinfo = (Get-Content (Join-Path $PSScriptRoot "appinfo.template.ini") -Raw) `
    -replace "@PACKAGE_VERSION@", $packageVersion `
    -replace "@DISPLAY_VERSION@", $version
Set-Content -Path (Join-Path $pkg "App/AppInfo/appinfo.ini") -Value $appinfo -Encoding ascii

# launcher.ini 双拷贝：生效位置 + Other\Source（PA.c 规范要求随包携带源配置）
foreach ($dst in @("App/AppInfo/Launcher/launcher.ini", "Other/Source/launcher.ini")) {
    Copy-Item (Join-Path $PSScriptRoot "launcher.ini") (Join-Path $pkg $dst)
}

# 图标：PA.c 要求 AppInfo 下 appicon.ico + appicon_{16,32,75,128,256}.png
Copy-Item (Join-Path $repoRoot "src-tauri/icons/icon.ico") (Join-Path $pkg "App/AppInfo/appicon.ico")
foreach ($size in @(16, 32, 75, 128, 256)) {
    Copy-Item (Join-Path $PSScriptRoot "icons/appicon_$size.png") (Join-Path $pkg "App/AppInfo/appicon_$size.png")
}

Copy-Item (Join-Path $PSScriptRoot "help.html") (Join-Path $pkg "help.html")

Write-Host "portable 包已组装: $pkg (PackageVersion=$packageVersion)"
