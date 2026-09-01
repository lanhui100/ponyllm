# ==============================================================================
# ponyllm 一键安装脚本 (Windows PowerShell)
# 用法: irm https://raw.githubusercontent.com/lanhui100/ponyllm/main/install.ps1 | iex
# ==============================================================================
$ErrorActionPreference = "Stop"

[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 -bor [Net.SecurityProtocolType]::Tls13

$Repo = "lanhui100/ponyllm"
$AssetName = "ponyllm-windows-x86_64.zip"
$DownloadUrl = "https://github.com/$Repo/releases/latest/download/$AssetName"

$InstallDir = Join-Path $HOME ".ponyllm\bin"
$ExePath = Join-Path $InstallDir "ponyllm.exe"

Write-Host "========================================================" -ForegroundColor Cyan
Write-Host "  正在安装 ponyllm (大模型统一网关与管理服务)" -ForegroundColor Green
Write-Host "  平台: Windows x86_64" -ForegroundColor Cyan
Write-Host "========================================================"

# 1. 确保安装目录存在
if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}

# 2. 下载发布压缩包
$TempZip = Join-Path ([System.IO.Path]::GetTempPath()) "ponyllm-installer-$([System.Guid]::NewGuid()).zip"
$TempExtract = Join-Path ([System.IO.Path]::GetTempPath()) "ponyllm-extract-$([System.Guid]::NewGuid())"

try {
    Write-Host "--> 正在从 GitHub Releases 下载最新版本..." -ForegroundColor Yellow
    if (Get-Command curl.exe -ErrorAction SilentlyContinue) {
        & curl.exe -fSL "$DownloadUrl" -o "$TempZip"
    } else {
        Invoke-WebRequest -Uri $DownloadUrl -OutFile $TempZip -UseBasicParsing
    }

    Write-Host "--> 正在解压发布包..." -ForegroundColor Yellow
    Expand-Archive -Path $TempZip -DestinationPath $TempExtract -Force

    $ExtractedExe = Join-Path $TempExtract "ponyllm.exe"
    if (-not (Test-Path $ExtractedExe)) {
        # 支持可能嵌套的解压目录
        $Found = Get-ChildItem -Path $TempExtract -Filter "ponyllm.exe" -Recurse | Select-Object -First 1
        if ($Found) {
            $ExtractedExe = $Found.FullName
        } else {
            throw "解压后未找到可执行文件 ponyllm.exe"
        }
    }

    Copy-Item -Path $ExtractedExe -Destination $ExePath -Force
    Write-Host "--> ponyllm 已成功安装至: $ExePath" -ForegroundColor Green
}
finally {
    if (Test-Path $TempZip) { Remove-Item -Path $TempZip -Force -ErrorAction SilentlyContinue }
    if (Test-Path $TempExtract) { Remove-Item -Path $TempExtract -Recurse -Force -ErrorAction SilentlyContinue }
}

# 3. 检查并配置 PATH 环境变量
$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($UserPath -notlike "*$InstallDir*") {
    Write-Host "--> 正在将 $InstallDir 添加到用户 PATH 环境变量..." -ForegroundColor Yellow
    $NewPath = if ([string]::IsNullOrWhiteSpace($UserPath)) { $InstallDir } else { "$UserPath;$InstallDir" }
    [Environment]::SetEnvironmentVariable("Path", $NewPath, "User")
    $env:Path = "$env:Path;$InstallDir"
    Write-Host "--> PATH 环境变量已自动更新！" -ForegroundColor Green
}

Write-Host "========================================================" -ForegroundColor Cyan
Write-Host "  ponyllm 安装完成！" -ForegroundColor Green
Write-Host ""
Write-Host "  快速上手:" -ForegroundColor White
Write-Host "    ponyllm init      # 生成默认配置文件 ponyllm.toml" -ForegroundColor Gray
Write-Host "    ponyllm serve     # 启动网关服务 (默认 http://127.0.0.1:8080)" -ForegroundColor Gray
Write-Host "    ponyllm status    # 巡检运行中网关状态与 QPS 指标" -ForegroundColor Gray
Write-Host "    ponyllm telemetry # 查看黑匣子故障录波记录" -ForegroundColor Gray
Write-Host "========================================================" -ForegroundColor Cyan
