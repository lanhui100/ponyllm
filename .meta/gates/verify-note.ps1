# verify-note.ps1 —— verify-note.sh 的 Windows/PowerShell 等价影子校验
# 覆盖 bash 版可机械校验的子集：1.3/1.4 两轴封闭集、1.5 深度、1.6 文件名与日期、
# 1.7 Status 与 lifecycle 一致、1.8 骨架标题。退出码：0 全过 / 1 有违例。
$ErrorActionPreference = 'Stop'
$notes = Join-Path $PSScriptRoot '..\..\.agents\notes'
$lifecycles = 'proposed','implemented','rejected','archived'
$classes = 'feature','bug-fix','simplification','architecture','process','testing'
$extra = @()
$classesLocal = Join-Path $notes 'classes.local'
if (Test-Path $classesLocal) {
  $extra = Get-Content $classesLocal | Where-Object { $_ -match '^[a-z0-9][a-z0-9-]*$' }
}
$classesAll = $classes + $extra
$fail = 0
function Fail($msg) { Write-Host "FAIL  $msg"; $script:fail = 1 }

Get-ChildItem $notes -Directory | ForEach-Object {
  if ($lifecycles -notcontains $_.Name) { Fail "1.3 顶层目录越界：$($_.Name)" }
}
Get-ChildItem $notes -Directory | ForEach-Object {
  $lc = $_.Name
  Get-ChildItem $_.FullName -Directory -ErrorAction SilentlyContinue | ForEach-Object {
    if ($classesAll -notcontains $_.Name) { Fail "1.4 class 越界：$lc/$($_.Name)" }
  }
}
Get-ChildItem $notes -Recurse -File -Filter *.md | ForEach-Object {
  $f = $_.FullName
  if ($_.Name -eq 'README.md') { return }
  $rel = $f.Substring((Resolve-Path $notes).Path.Length + 1)
  $parts = $rel -split '[\\/]'
  if ($parts.Count -ne 3) { Fail "1.5 $f：未恰好位于 {lifecycle}/{class}/ 两级"; return }
  $lc = $parts[0]
  if ($_.Name -notmatch '^\d{4}-\d{2}-\d{2}-.+\.md$') { Fail "1.6 $f：文件名不符 yyyy-mm-dd-topic.md" }
  else {
    $d = [datetime]::MinValue
    if (-not [datetime]::TryParseExact($_.Name.Substring(0,10),'yyyy-MM-dd',[System.Globalization.CultureInfo]::InvariantCulture,[System.Globalization.DateTimeStyles]::None,[ref]$d)) {
      Fail "1.6 $f：日期不合法（$($_.Name.Substring(0,10))）"
    }
  }
  $statusLine = (Select-String -Path $f -Pattern '^Status:\s*(\S+)' | Select-Object -First 1)
  if (-not $statusLine) { Fail "1.7 $f：缺 Status: 行" }
  else {
    $st = $statusLine.Matches[0].Groups[1].Value
    if ($lc -eq 'archived') { if ($st -ne 'implemented' -and $st -ne 'archived') { Fail "1.7 $f：archived 下 Status 应为 implemented/archived，实得 $st" } }
    elseif ($st -ne $lc) { Fail "1.7 $f：Status($st) 与目录($lc) 不一致" }
  }
  $headings = Get-Content $f | Where-Object { $_ -match '^## ' }
  $hasProposal = $headings | Where-Object { $_ -match '^## Proposal(\s|$)' }
  $hasDecision = $headings | Where-Object { $_ -match '^## Decision(\s|$)' }
  $hasProposalEra = $headings | Where-Object { $_ -match '^## (Proposal|Plan|Migration plan|Acceptance criteria)(\s|$)' }
  switch ($lc) {
    'implemented' { if ($hasProposalEra) { Fail "1.8 $f：implemented 含提案时代标题" } }
    { $_ -in 'proposed','rejected' } {
      if (-not $hasProposal) { Fail "1.8 $f：$lc 缺 ## Proposal" }
      if ($hasDecision) { Fail "1.8 $f：$lc 含现在时 ## Decision（提案伪装成决定）" }
    }
  }
}
if ($fail -eq 0) { Write-Host 'verify-note.ps1: 全部通过'; exit 0 } else { exit 1 }
