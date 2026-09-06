# check-tasks.ps1 —— 任务系统门禁（manage-tasks skill 的验证脚本）
# Active 区（03_TASKS）禁终态：Done/Dropped 须随完工提交同刻归档进 04_ARCHIVED。
# Board Done/Dropped 区为归档备查（只放已归档 ID）：验归档存在 + Done 须 review +
# 归档卡状态终态；Dashboard 只链 Active ID。
# FAIL（exit 1，确定性高、无假阳性）：卡结构/字段/ID/状态集/终态隔离/ADR单源与存在/
# 幽灵ID/Board列态一致/归档备查三件/重复ID/ADR改名同提交/Done窗超限。
# WARN（exit 0，启发式）：WIP上限/陈旧卡/Owner待定/Dashboard滞后/日志断档/收口未清。
# 收口判定用 git cherry（squash 等价已合）：cherry 无 + 行即已合；--merged 在 squash 下恒空，不用。
# 语义靠 review（不进门禁）：验收质量、状态真实性、Output允许指向尚不存在的未来文件。
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$tasksDir = Join-Path $root '.agents\tasks\web-console\03_TASKS'
$archDir = Join-Path $root '.agents\tasks\web-console\04_ARCHIVED'
$reviewsDir = Join-Path $root '.agents\tasks\web-console\02_REVIEWS'
$logsDir = Join-Path $root '.agents\tasks\web-console\99_LOGS'
$board = Join-Path $root '.agents\tasks\web-console\01_TASK_BOARD.md'
$dash = Join-Path $root '.agents\tasks\web-console\00_DASHBOARD.md'
$fail = 0
function Fail($msg) { Write-Host "FAIL  $msg"; $script:fail = 1 }
function Warn($msg) { Write-Host "WARN  $msg" }

$requiredSections = @('## Basic Info','## Goal','## Output','## Acceptance Criteria','## Current Progress','## Next Action','## Resume Hint','## Review Summary','## Related Files')
$taskFiles = @(Get-ChildItem $tasksDir -File -Filter *.md -ErrorAction SilentlyContinue)
$ids = @()
$statusOf = @{}
$branchOf = @{}
foreach ($f in $taskFiles) {
  $text = Get-Content $f.FullName -Raw
  foreach ($sec in $requiredSections) {
    if ($text -notmatch [regex]::Escape($sec)) { Fail "$($f.Name)：缺 $sec 节" }
  }
  $mId = [regex]::Match($text, '(?m)^- ID:\s*(\S+)')
  if (-not $mId.Success) { Fail "$($f.Name)：缺 - ID: 行"; continue }
  $id = $mId.Groups[1].Value
  if ($ids -contains $id) { Fail "$($f.Name)：ID($id)重复" }
  $ids += $id
  $expectPrefix = ($f.BaseName -split '-', 3)[0] + '-' + ($f.BaseName -split '-', 3)[1]
  if ($id -ne $expectPrefix) { Fail "$($f.Name)：ID($id)与文件名前缀($expectPrefix)不一致" }
  foreach ($field in @('Status','Priority','Created At','Updated At')) {
    if ($text -notmatch "(?m)^- $([regex]::Escape($field)):\s*\S") { Fail "$($f.Name)：缺 - ${field}: 行" }
  }
  $mSt = [regex]::Match($text, '(?m)^- Status:\s*(.+?)\s*$')
  $st = ''
  if ($mSt.Success) { $st = $mSt.Groups[1].Value.Trim() }
  $statusOf[$id] = $st
  $mBr = [regex]::Match($text, '(?m)^- Branch:\s*(\S+)')
  $branchOf[$id] = if ($mBr.Success) { $mBr.Groups[1].Value.Trim() } else { '' }
  if ($st -in @('In Progress','Review') -and $branchOf[$id] -notmatch '^task/\S+') {
    Warn "$($f.Name)：$st 缺 - Branch: task/WEB-XX-slug 行，并行认领不可查"
  }
  if ($st -match '^(Done|Dropped)') { Fail "$($f.Name)：03_TASKS 为 Active 区，终态($st)须随完工提交同刻 git mv 进 04_ARCHIVED，不在 Active 停留" }
  $allowed = @('Backlog','Ready','In Progress','Review','Blocked')
  if ($allowed -notcontains $st) { Fail "$($f.Name)：Status($st)不在允许集" }
  $relSec = ''
  if ($text -match '(?s)## Related Files\s*(.*?)(?:\r?\n## |\z)') { $relSec = $Matches[1] }
  $adrLinks = [regex]::Matches($relSec, '(?m)^- ADR:\s*`?(.+?\.md)`?\s*$')
  if ($adrLinks.Count -eq 0) { Fail "$($f.Name)：Related Files缺 - ADR: 链接" }
  foreach ($m in $adrLinks) {
    $p = $m.Groups[1].Value.Trim().Trim('`','"').Replace('/', '\')
    if (-not (Test-Path (Join-Path $root $p))) { Fail "$($f.Name)：ADR链接不存在 $p" }
  }
  foreach ($secName in @('Next Action','Resume Hint')) {
    $secText = ''
    if ($text -match "(?s)## $([regex]::Escape($secName))\s*(.*?)(?:\r?\n## |\z)") { $secText = $Matches[1] }
    $norm = ($secText -replace '[\s\-\*\d\.、>]+','')
    if ($norm.Length -eq 0) { Fail "$($f.Name)：$secName 为空" }
    elseif ($norm -match '^(继续推进|后续再看|待处理)$') { Fail "$($f.Name)：$secName 为空话（$norm），须写可执行动作" }
  }
  $resumeSec = ''
  if ($text -match '(?s)## Resume Hint\s*(.*?)(?:\r?\n## |\z)') { $resumeSec = $Matches[1] }
  if ($resumeSec -match '\.agents/notes/') { Fail "$($f.Name)：Resume Hint含ADR路径，唯一真源为Related Files" }
  if ($st -eq 'In Progress') {
    $mUp = [regex]::Match($text, '(?m)^- Updated At:\s*(\d{4}-\d{2}-\d{2})')
    if ($mUp.Success) {
      try {
        $age = ((Get-Date) - [datetime]::ParseExact($mUp.Groups[1].Value,'yyyy-MM-dd',$null)).Days
        if ($age -gt 14) { Warn "$($f.Name)：In Progress已$age 天未更新，确认是否阻塞" }
      } catch { }
    }
    if ($text -match '(?m)^- Owner:\s*待定\s*$') { Warn "$($f.Name)：In Progress但Owner待定，请认领" }
  }
}
$archIds = @()
if (Test-Path $archDir) {
  foreach ($af in @(Get-ChildItem $archDir -File -Filter *.md -ErrorAction SilentlyContinue)) {
    foreach ($am in [regex]::Matches($af.BaseName, 'WEB-\d+')) {
      if ($archIds -notcontains $am.Value) { $archIds += $am.Value }
    }
    $atext = Get-Content $af.FullName -Raw
    $amSt = [regex]::Match($atext, '(?m)^- Status:\s*(.+?)\s*$')
    if ($amSt.Success) {
      $ast = $amSt.Groups[1].Value.Trim()
      if ($ast -notin @('Done','Dropped')) { Fail "$($af.Name)：归档卡状态($ast)非终态，归档只收 Done/Dropped" }
    }
  }
}
$colMap = @{ 'Backlog'='Backlog'; 'Ready'='Ready'; 'In Progress'='In Progress'; 'Review'='Review'; 'Blocked'='Blocked'; 'Done'='Done'; 'Dropped'='Dropped' }
foreach ($ref in @($board, $dash)) {
  if (-not (Test-Path $ref)) { Fail "缺文件 $ref"; continue }
  $t = Get-Content $ref -Raw
  $isBoard = ([IO.Path]::GetFileName($ref) -eq '01_TASK_BOARD.md')
  foreach ($m in [regex]::Matches($t, 'WEB-\d+')) {
    if ($ids -contains $m.Value) { continue }
    if ($isBoard -and ($archIds -contains $m.Value)) { continue }
    if (-not $isBoard -and ($archIds -contains $m.Value)) { Fail "$([IO.Path]::GetFileName($ref))引用已归档ID $($m.Value)（Dashboard 只链 Active，备查见 Board Done 区）"; continue }
    Fail "$([IO.Path]::GetFileName($ref))引用幽灵ID $($m.Value)"
  }
}
if (Test-Path $board) {
  $lines = Get-Content $board
  $cur = ''
  $doneCount = 0
  $colCounts = @{}
  foreach ($ln in $lines) {
    $hm = [regex]::Match($ln, '^##\s*(.+?)\s*$')
    if ($hm.Success) {
      $h = $hm.Groups[1].Value
      $cur = ''
      foreach ($k in $colMap.Keys) { if ($h -eq $k -or $h.StartsWith($k)) { $cur = $colMap[$k] } }
      continue
    }
    if ($cur -eq '') { continue }
    foreach ($m in [regex]::Matches($ln, 'WEB-\d+')) {
      $wid = $m.Value
      if ($cur -in @('Done','Dropped')) {
        if ($statusOf.ContainsKey($wid)) { Fail "Board $cur 区 $wid 仍是 Active 卡（Status=$($statusOf[$wid])），先归档再留备查行" }
        elseif ($archIds -notcontains $wid) { Fail "Board $cur 区 $wid 无 04_ARCHIVED 归档（先归档再留备查行）" }
        elseif ($cur -eq 'Done' -and -not (Test-Path (Join-Path $reviewsDir "$wid*.md"))) { Fail "Board Done 备查 $wid 缺 02_REVIEWS/$wid 审核文件" }
        if ($cur -eq 'Done') {
          $doneCount++
          $archFile = @(Get-ChildItem $archDir -File -Filter "*$wid*.md" -ErrorAction SilentlyContinue | Select-Object -First 1)
          if ($archFile.Count -gt 0) {
            $archText = Get-Content $archFile[0].FullName -Raw
            if ($archText -notmatch '(?m)^- Merge:\s*\S+') { Warn "Board Done 备查 ${wid}：归档卡缺 - Merge 注记，合后补 merge SHA" }
          }
        }
        continue
      }
      if ($colCounts.ContainsKey($cur)) { $colCounts[$cur]++ } else { $colCounts[$cur] = 1 }
      if ($statusOf.ContainsKey($wid) -and $statusOf[$wid] -ne $cur -and $cur -in @('Backlog','Ready','In Progress','Review','Blocked')) {
        Fail "Board列($cur)与卡状态($($statusOf[$wid]))不一致：$wid"
      }
    }
  }
  if ($doneCount -gt 5) { Fail "Board Done备查$doneCount 条超窗(>5)，请删最旧备查行（卡已在 04_ARCHIVED，无损）" }
  if (($colCounts['In Progress'] -gt 2)) { Warn "In Progress $($colCounts['In Progress']) 超上限2，先清再开" }
  if (($colCounts['Review'] -gt 3)) { Warn "Review $($colCounts['Review']) 超上限3，先审再推" }
}
try {
  $br = (git symbolic-ref --short HEAD 2>$null).Trim()
  if ($br -match '^task/(WEB-\d+)-') {
    $bid = $Matches[1]
    if ($ids -notcontains $bid) { Warn "当前分支$br 无对应任务卡（幽灵分支）" }
    elseif ($statusOf[$bid] -notin @('In Progress','Review')) { Warn "分支$br 对应卡状态为$($statusOf[$bid])，非In Progress/Review" }
    elseif ($branchOf[$bid] -ne $br) { Warn "分支$br 与卡登记($($branchOf[$bid]))不一致" }
  }
} catch { }
try {
  $staged = git diff --cached --name-status 2>$null
  $adrRename = $staged | Where-Object { $_ -match '^R\d+\s+\.agents/notes/' }
  if ($adrRename) {
    $taskStaged = $staged | Where-Object { $_ -match '\.agents/tasks/' }
    if (-not $taskStaged) { Fail "ADR改名已暂存但无任务更新暂存，须同提交更新链接" }
  }
} catch { }
try {
  git rev-parse --verify --quiet main 2>$null | Out-Null
  if ($LASTEXITCODE -eq 0) {
    $tb = git branch --format='%(refname:short)' --list 'task/WEB-*' 2>$null
    foreach ($mb in $tb) {
      $b = $mb.Trim()
      if (-not $b) { continue }
      $ch = git cherry main $b 2>$null
      if (-not ($ch -match '^\+')) { Warn "已合分支未清 $b（cherry 为空即等价已合，含 squash；收口：先worktree remove再branch -D，仅此条件允许 -D）" }
    }
  }
} catch { }
try {
  $logFiles = @(Get-ChildItem $logsDir -File -Filter *.md -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending)
  if ($logFiles.Count -gt 0 -and ((Get-Date) - $logFiles[0].LastWriteTime).Days -gt 7) {
    Warn "会话日志断档$([int]((Get-Date)-$logFiles[0].LastWriteTime).Days)天，收尾请补99_LOGS"
  }
} catch { }
if ($fail -eq 0) { Write-Host 'check-tasks.ps1: 全部通过（WARN见上，靠review项未进门禁）'; exit 0 } else { exit 1 }
