param(
    [string]$Source = "",
    [string]$Target = "C:\PekRDbProxy",
    [switch]$RegisterAgent,
    [switch]$SkipService
)

# Pek.RDbProxy 一键安装（Windows）
#   1) 停旧服务 → 复制程序 → 首次生成配置模板
#   2) 注册并启动 Windows 服务（PekRDbProxy）
#   3) -RegisterAgent：注册到星尘（影子模式，之后升级=直接覆盖程序文件即自动重启）
#
# 用法：
#   .\install.ps1                          # 用同目录的 pek-rdbproxy.exe 安装到 C:\PekRDbProxy
#   .\install.ps1 -Target D:\Apps\RDbProxy # 自定义安装目录
#   .\install.ps1 -RegisterAgent           # 同时注册到星尘
#   .\install.ps1 -Source .\pek-rdbproxy.exe

$ErrorActionPreference = 'Stop'
$exeName = 'pek-rdbproxy.exe'

if (-not $Source) { $Source = Join-Path $PSScriptRoot $exeName }
if (-not (Test-Path $Source)) {
    Write-Host "找不到程序：$Source" -ForegroundColor Red
    Write-Host "请把 $exeName 与本脚本放在同一目录，或用 -Source 指定路径。" -ForegroundColor Yellow
    exit 1
}
$Source = (Resolve-Path $Source).Path

Write-Host "== Pek.RDbProxy 安装 ==" -ForegroundColor Cyan
Write-Host "  源程序：$Source"
Write-Host "  安装到：$Target"

# 1) 停掉正在运行的服务/进程（避免 exe 被占用）
$svc = Get-Service -Name 'PekRDbProxy' -ErrorAction SilentlyContinue
if ($svc) {
    Write-Host "  停止旧服务…"
    if ($svc.Status -ne 'Stopped') { try { Stop-Service -Name 'PekRDbProxy' -Force } catch {} }
    Get-Process 'pek-rdbproxy' -ErrorAction SilentlyContinue | ForEach-Object {
        try { Wait-Process -Id $_.Id -Timeout 10 } catch {}
    }
}
Get-Process 'pek-rdbproxy' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue

# 2) 复制程序
if (-not (Test-Path $Target)) { New-Item -ItemType Directory -Path $Target -Force | Out-Null }
$dest = Join-Path $Target $exeName
Write-Host "  复制程序…"
for ($i = 0; $i -lt 20; $i++) {
    try { Copy-Item -Path $Source -Destination $dest -Force; break }
    catch {
        if ($i -eq 19) { throw }
        Start-Sleep -Milliseconds 500
    }
}

# 3) 首次生成配置（不覆盖已有配置）
$config = Join-Path $Target 'Config\Proxy.toml'
if (-not (Test-Path $config)) {
    Write-Host "  生成配置模板…"
    & $dest -init | Out-Null
} else {
    Write-Host "  保留已有配置：$config"
}

# 4) 注册并启动服务
if (-not $SkipService) {
    Write-Host "  注册 Windows 服务…"
    & $dest -install | Out-Null
    Start-Sleep -Seconds 1
    Write-Host "  启动服务…"
    & $dest -start | Out-Null
    Start-Sleep -Seconds 2
    $svc = Get-Service -Name 'PekRDbProxy' -ErrorAction SilentlyContinue
    if ($svc) { Write-Host ("  服务状态：" + $svc.Status) -ForegroundColor Green }
}

# 5) 注册到星尘
if ($RegisterAgent) {
    Write-Host "  注册到星尘（影子模式）…"
    & $dest -RegisterAgent
}

Write-Host ""
Write-Host "安装完成。" -ForegroundColor Green
Write-Host "  安装目录：$Target"
Write-Host "  配置文件：$config（连接串已用 Config/.key 加密）"
Write-Host "  面板地址：http://127.0.0.1:5599/  （首次账号 admin/admin，登录后请改密）"
Write-Host ""
Write-Host "后续操作：" -ForegroundColor Cyan
Write-Host "  查看状态：$dest -status"
Write-Host "  停止/启动：$dest -stop / -start"
Write-Host "  注册星尘：$dest -RegisterAgent"
Write-Host "  升级：把新程序放进 $Target\Update\（10 秒内自动替换并重启）"
