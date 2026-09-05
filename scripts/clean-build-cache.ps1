<#
.SYNOPSIS
    ponyllm 构建产物与缓存清理脚本
.DESCRIPTION
    清理 target 目录并显示释放空间。
#>
[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent $PSScriptRoot

function Get-DirSizeMB ($path) {
    if (Test-Path -LiteralPath $path) {
        $measure = Get-ChildItem -LiteralPath $path -Recurse -Force -File -ErrorAction SilentlyContinue | Measure-Object -Property Length -Sum
        if ($measure.Sum) {
            return [math]::Round($measure.Sum / 1MB, 2)
        }
    }
    return 0
}

Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  ponyllm 构建产物清理工具" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan

$rootTarget = Join-Path $projectRoot "target"
$rootSizeBefore = Get-DirSizeMB $rootTarget

Write-Host "[1/2] 正在清理 Rust 构建产物 (当前: $rootSizeBefore MB)..." -ForegroundColor Yellow
Push-Location $projectRoot
try {
    cargo clean
} finally {
    Pop-Location
}

Write-Host "[2/2] 统计清理结果..." -ForegroundColor Green
Write-Host "----------------------------------------"
Write-Host ("已释放构建缓存: {0:N2} MB ({1:N2} GB)" -f $rootSizeBefore, ($rootSizeBefore / 1024)) -ForegroundColor Green

$drive = Get-PSDrive D -ErrorAction SilentlyContinue
if ($drive) {
    $freeGB = [math]::Round($drive.Free / 1GB, 2)
    Write-Host ("当前 D 盘剩余可用空间: {0} GB" -f $freeGB) -ForegroundColor Cyan
}
Write-Host "========================================" -ForegroundColor Cyan