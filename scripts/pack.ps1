param(
    [ValidateSet('windows', 'linux', 'all')][string]$Targets = 'all',
    [switch]$NoBump,
    [switch]$SkipBuild
)

# Pek.RDbProxy 打包
#   - 默认打包前把补丁版本 +1（每次产出的包版本唯一）
#   - Linux 一律走 zig 交叉编译（musl 静态单文件）：x64 + arm64
#   - 产物：dist/pek-rdbproxy-v<版本>-<平台>.zip|tar.gz + SHA256SUMS
#
# 用法：
#   .\scripts\pack.ps1                 # 全部平台（web 服务用不上 macOS，故不含）
#   .\scripts\pack.ps1 -Targets linux
#   .\scripts\pack.ps1 -NoBump -SkipBuild

$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..')
Push-Location $root
try {
    # ── 前端语法门禁（app.js 解析失败会让整块面板函数都不存在，如 doLogin；有 node 才检查）
    $appJs = Join-Path $root 'res\app.js'
    if ((Test-Path $appJs) -and (Get-Command node -ErrorAction SilentlyContinue)) {
        & node --check $appJs
        if ($LASTEXITCODE -ne 0) { throw 'res/app.js 语法错误（node --check 未通过），已中止打包' }
        Write-Host '前端语法检查：app.js 通过' -ForegroundColor DarkGray
    }

    # ── 版本
    $cargoPath = Join-Path $root 'Cargo.toml'
    $cargo = [IO.File]::ReadAllText($cargoPath, [Text.Encoding]::UTF8)
    $m = [regex]::Match($cargo, '(?m)^version\s*=\s*"([^"]+)"')
    if (-not $m.Success) { throw 'Cargo.toml 里没有 version' }
    $version = $m.Groups[1].Value
    if (-not $NoBump) {
        $parts = $version.Split('.')
        if ($parts.Length -lt 3) { throw "版本号不是 主.次.修订：$version" }
        $parts[2] = [string]([int]$parts[2] + 1)
        $newVersion = $parts -join '.'
        $cargo = [regex]::Replace($cargo, '(?m)^version\s*=\s*"' + [regex]::Escape($version) + '"', "version = `"$newVersion`"", 1)
        [IO.File]::WriteAllText($cargoPath, $cargo, (New-Object Text.UTF8Encoding($false)))
        Write-Host "版本：$version → $newVersion" -ForegroundColor Cyan
        $version = $newVersion
    } else {
        Write-Host "版本：$version（未递增）" -ForegroundColor Cyan
    }

    $dist = Join-Path $root 'dist'
    if (-not (Test-Path $dist)) { New-Item -ItemType Directory -Path $dist | Out-Null }

    $built = @()   # @{ Name; Path }
    $doWindows = ($Targets -eq 'windows' -or $Targets -eq 'all')
    $doLinux = ($Targets -eq 'linux' -or $Targets -eq 'all')

    # ── Windows
    if ($doWindows) {
        if (-not $SkipBuild) {
            Write-Host "构建 windows-x64（release）…" -ForegroundColor Yellow
            & cargo build --release --offline
            if ($LASTEXITCODE -ne 0) { throw 'Windows 构建失败' }
        }
        $built += @{ Name = 'win-x64'; Path = (Join-Path $root 'target\release\pek-rdbproxy.exe') }
    }

    # ── Linux（zig 交叉）
    if ($doLinux) {
        $linuxTargets = @(
            @{ Name = 'linux-x64'; Triple = 'x86_64-unknown-linux-musl' },
            @{ Name = 'linux-arm64'; Triple = 'aarch64-unknown-linux-musl' }
        )
        if (-not $SkipBuild) {
            $zig = $env:CARGO_ZIGBUILD_ZIG_PATH
            if (-not $zig) {
                $searchDirs = @('E:\Soft', 'G:\Tools\zig', (Join-Path $root 'tools'))
                foreach ($dir in $searchDirs) {
                    if (-not (Test-Path $dir)) { continue }
                    $hit = @(Get-ChildItem -Path $dir -Filter 'zig-*' -Directory -ErrorAction SilentlyContinue |
                        Sort-Object Name -Descending)
                    if ($hit.Count -gt 0) {
                        $exe = Join-Path $hit[0].FullName 'zig.exe'
                        if (Test-Path $exe) { $zig = $exe; break }
                    }
                }
            }
            if (-not $zig -or -not (Test-Path $zig)) { throw '找不到 zig（设置 CARGO_ZIGBUILD_ZIG_PATH，或把 zig 放到 E:\Soft\zig-*）' }
            $env:CARGO_ZIGBUILD_ZIG_PATH = $zig
            Write-Host "zig：$zig" -ForegroundColor DarkGray
            if (-not (Get-Command cargo-zigbuild -ErrorAction SilentlyContinue)) {
                throw '缺少 cargo-zigbuild：cargo install cargo-zigbuild'
            }
        }
        foreach ($t in $linuxTargets) {
            if (-not $SkipBuild) {
                Write-Host ("构建 {0}（{1}）…" -f $t.Name, $t.Triple) -ForegroundColor Yellow
                & cargo zigbuild --release --offline --target $t.Triple
                if ($LASTEXITCODE -ne 0) { throw ("{0} 构建失败" -f $t.Name) }
            }
            $bin = Join-Path $root ("target\{0}\release\pek-rdbproxy" -f $t.Triple)
            $built += @{ Name = $t.Name; Path = $bin }
        }
    }

    # ── 打包
    $sums = @()
    foreach ($item in $built) {
        if (-not (Test-Path $item.Path)) { throw ("产物不存在：" + $item.Path) }
        $stem = "pek-rdbproxy-v$version-$($item.Name)"
        if ($item.Name -like 'win-*') {
            $stage = Join-Path $dist "stage-$($item.Name)"
            if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
            New-Item -ItemType Directory -Path $stage | Out-Null
            Copy-Item $item.Path (Join-Path $stage 'pek-rdbproxy.exe')
            Copy-Item (Join-Path $root 'scripts\install.ps1') (Join-Path $stage 'install.ps1')
            Copy-Item (Join-Path $root 'README.md') (Join-Path $stage 'README.md') -ErrorAction SilentlyContinue
            $zip = Join-Path $dist "$stem.zip"
            if (Test-Path $zip) { Remove-Item $zip -Force }
            Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
            Remove-Item $stage -Recurse -Force
            $sums += $zip
        } else {
            $stage = Join-Path $dist "stage-$($item.Name)"
            if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
            New-Item -ItemType Directory -Path $stage | Out-Null
            Copy-Item $item.Path (Join-Path $stage 'pek-rdbproxy')
            Copy-Item (Join-Path $root 'scripts\install.sh') (Join-Path $stage 'install.sh')
            Copy-Item (Join-Path $root 'README.md') (Join-Path $stage 'README.md') -ErrorAction SilentlyContinue
            $tar = Join-Path $dist "$stem.tar.gz"
            if (Test-Path $tar) { Remove-Item $tar -Force }
            # Git 自带 tar（bsdtar 不支持 --force-local）：正斜杠路径 + usr\bin 进 PATH（tar 调 gzip）
            $gitBin = 'C:\Program Files\Git\usr\bin'
            $tarExe = Join-Path $gitBin 'tar.exe'
            $useGitTar = Test-Path $tarExe
            if ($useGitTar) {
                if ($env:PATH -notlike "*$gitBin*") { $env:PATH = "$gitBin;$env:PATH" }
            } else {
                $tarExe = 'tar'
            }
            $stageFwd = $stage.Replace('\', '/')
            $tarFwd = $tar.Replace('\', '/')
            if ($useGitTar) {
                & $tarExe --force-local -czf $tarFwd -C $stageFwd .
            } else {
                & $tarExe -czf $tarFwd -C $stageFwd .
            }
            if ($LASTEXITCODE -ne 0) { throw ("打包失败：" + $item.Name) }
            Remove-Item $stage -Recurse -Force
            $sums += $tar
        }
        Write-Host ("  产出：" + (Split-Path $item.Path -Leaf) + " → " + (Split-Path $sums[-1] -Leaf)) -ForegroundColor Green
    }

    # ── SHA256SUMS
    $lines = foreach ($f in $sums) {
        $h = (Get-FileHash -Path $f -Algorithm SHA256).Hash
        "{0}  {1}" -f $h, (Split-Path $f -Leaf)
    }
    $sumFile = Join-Path $dist 'SHA256SUMS'
    [IO.File]::WriteAllText($sumFile, (($lines -join "`n") + "`n"), (New-Object Text.UTF8Encoding($false)))
    Write-Host ""
    Write-Host "完成，产物在 $dist" -ForegroundColor Cyan
    $lines | ForEach-Object { Write-Host "  $_" }
}
finally {
    Pop-Location
}
