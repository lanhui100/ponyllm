#!/usr/bin/env bash
# check-tasks.sh —— check-tasks.ps1 的 bash 等价影子（CI/Linux 用）
# Active 区（03_TASKS）禁终态：Done/Dropped 须随完工提交同刻归档进 04_ARCHIVED。
# Board Done/Dropped 区为归档备查（只放已归档 ID）：验归档存在 + Done 须 review +
# 归档卡状态终态；Dashboard 只链 Active ID。
# FAIL exit 1 / WARN 只提示 exit 0。语义靠 review，不进门禁。
# 收口判定用 git cherry（squash 等价已合）；--merged 在 squash 下恒空，不用。
# 本机 Windows 无 bash 在 PATH 时以 review 保证与 .ps1 对齐，CI 首绿为准。
set -u
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TASKS_DIR="$ROOT_DIR/.agents/tasks/web-console/03_TASKS"
ARCH_DIR="$ROOT_DIR/.agents/tasks/web-console/04_ARCHIVED"
REVIEWS_DIR="$ROOT_DIR/.agents/tasks/web-console/02_REVIEWS"
BOARD="$ROOT_DIR/.agents/tasks/web-console/01_TASK_BOARD.md"
DASH="$ROOT_DIR/.agents/tasks/web-console/00_DASHBOARD.md"
FAIL=0
fail() { printf 'FAIL  %s\n' "$*" >&2; FAIL=1; }
warn() { printf 'WARN  %s\n' "$*" >&2; }
SECTIONS=('## Basic Info' '## Goal' '## Output' '## Acceptance Criteria' '## Current Progress' '## Next Action' '## Resume Hint' '## Review Summary' '## Related Files')
IDS=""
ARCH_IDS=""
declare -A ST_OF
declare -A BR_OF
for f in "$TASKS_DIR"/*.md; do
  [ -e "$f" ] || continue
  b="$(basename "$f" .md)"
  expect="$(echo "$b" | grep -oE '^WEB-[0-9]+')"
  for s in "${SECTIONS[@]}"; do grep -qF "$s" "$f" || fail "$b：缺 $s 节"; done
  id="$(sed -n 's/^- ID:[[:space:]]*//p' "$f" | head -n1 | tr -d ' \r')"
  [ -n "$id" ] || { fail "$b：缺 - ID: 行"; continue; }
  echo "$IDS" | grep -qw "$id" && fail "$b：ID($id)重复"
  IDS="$IDS $id"
  [ "$id" = "$expect" ] || fail "$b：ID($id)与文件名前缀($expect)不一致"
  for fd in 'Status' 'Priority' 'Created At' 'Updated At'; do
    grep -qE "^- $fd:[[:space:]]*[^[:space:]]" "$f" || fail "$b：缺 - $fd: 行"
  done
  st="$(sed -n 's/^- Status:[[:space:]]*//p' "$f" | head -n1 | tr -d '\r' | sed 's/[[:space:]]*$//')"
  ST_OF["$id"]="$st"
  brreg="$(sed -n 's/^- Branch:[[:space:]]*//p' "$f" | head -n1 | tr -d '\r' | sed 's/[[:space:]]*$//')"
  BR_OF["$id"]="$brreg"
  case "$st" in Done*|Dropped*) fail "$b：03_TASKS 为 Active 区，终态($st)须随完工提交同刻 git mv 进 04_ARCHIVED";; esac
  case "$st" in Backlog|Ready|"In Progress"|Review|Blocked) ;; *) fail "$b：Status($st)不在允许集";; esac
  rel="$(sed -n '/^## Related Files/,/^## /p' "$f")"
  echo "$rel" | grep -qE '^- ADR: [`"]?.+\.md[`"]?' || fail "$b：Related Files缺 - ADR: 链接"
  while read -r p; do
    [ -n "$p" ] || continue
    [ -f "$ROOT_DIR/$p" ] || fail "$b：ADR链接不存在 $p"
  done < <(echo "$rel" | grep -oE '\.agents/notes/[^ `"'\''\)]+\.md' || true)
  for sec in 'Next Action' 'Resume Hint'; do
    body="$(sed -n "/^## $sec/,/^## /p" "$f" | sed '1d;$d')"
    norm="$(echo "$body" | tr -d ' \t\r\n-*0123456789.、>')"
    [ -n "$norm" ] || { fail "$b：$sec 为空"; continue; }
    case "$norm" in 继续推进|后续再看|待处理) fail "$b：$sec 为空话（$norm）";; esac
  done
  resume="$(sed -n '/^## Resume Hint/,/^## /p' "$f")"
  echo "$resume" | grep -q '\.agents/notes/' && fail "$b：Resume Hint含ADR路径"
done
if [ -d "$ARCH_DIR" ]; then
  for af in "$ARCH_DIR"/*.md; do
    [ -e "$af" ] || continue
    ab="$(basename "$af" .md)"
    for aid in $(echo "$ab" | grep -oE 'WEB-[0-9]+' || true); do
      echo "$ARCH_IDS" | grep -qw "$aid" || ARCH_IDS="$ARCH_IDS $aid"
    done
    ast="$(sed -n 's/^- Status:[[:space:]]*//p' "$af" | head -n1 | tr -d '\r' | sed 's/[[:space:]]*$//')"
    case "$ast" in Done*|Dropped*|"") ;; *) fail "$(basename "$af")：归档卡状态($ast)非终态，归档只收 Done/Dropped";; esac
  done
fi
for ref in "$BOARD" "$DASH"; do
  [ -f "$ref" ] || { fail "缺文件 $ref"; continue; }
  isboard=0; [ "$ref" = "$BOARD" ] && isboard=1
  while read -r wid; do
    [ -n "$wid" ] || continue
    if echo "$IDS" | grep -qw "$wid"; then continue; fi
    if [ "$isboard" = "1" ] && echo "$ARCH_IDS" | grep -qw "$wid"; then continue; fi
    if [ "$isboard" = "0" ] && echo "$ARCH_IDS" | grep -qw "$wid"; then
      fail "$(basename "$ref")引用已归档ID $wid（Dashboard 只链 Active，备查见 Board Done 区）"; continue
    fi
    ls "$TASKS_DIR/$wid-"*.md >/dev/null 2>&1 && continue
    fail "$(basename "$ref")引用幽灵ID $wid"
  done < <(grep -oE 'WEB-[0-9]+' "$ref" 2>/dev/null | sort -u || true)
done
if [ -f "$BOARD" ]; then
  cur=""
  donec=0; ipc=0; revc=0
  while IFS= read -r ln; do
    if [[ "$ln" =~ ^##[[:space:]]+(.+)$ ]]; then
      h="${BASH_REMATCH[1]}"; cur=""
      case "$h" in Backlog|Ready|"In Progress"|Review|Blocked|Done*|Dropped*) cur="$h";; esac
      continue
    fi
    [ -n "$cur" ] || continue
    while read -r wid; do
      [ -n "$wid" ] || continue
      case "$cur" in Done*|Dropped*)
        if ls "$TASKS_DIR/$wid-"*.md >/dev/null 2>&1; then
          fail "Board $cur 区 $wid 仍是 Active 卡，先归档再留备查行"
        elif ! echo " $ARCH_IDS " | grep -q " $wid "; then
          fail "Board $cur 区 $wid 无 04_ARCHIVED 归档（先归档再留备查行）"
        else
          case "$cur" in Done*)
            ls "$REVIEWS_DIR/$wid"*.md >/dev/null 2>&1 || fail "Board Done 备查 $wid 缺 02_REVIEWS/$wid 审核文件"
            donec=$((donec+1))
            archf="$(ls "$ARCH_DIR"/*"$wid"*.md 2>/dev/null | head -n1 || true)"
            if [ -n "$archf" ]; then
              grep -qE '^- Merge:[[:space:]]*[^[:space:]]' "$archf" || warn "Board Done 备查 $wid：归档卡缺 - Merge 注记，合后补 merge SHA"
            fi ;;
          esac
        fi
        continue ;;
      esac
      case "$cur" in Done*) donec=$((donec+1));; "In Progress") ipc=$((ipc+1));; Review) revc=$((revc+1));; esac
      want="$cur"; [[ "$want" == Done* ]] && want="Done"; [[ "$want" == Dropped* ]] && want="Dropped"
      actual="${ST_OF[$wid]:-}"
      [ -z "$actual" ] && continue
      case "$want" in Backlog|Ready|"In Progress"|Review|Blocked)
        [ "$actual" = "$want" ] || fail "Board列($want)与卡状态($actual)不一致：$wid" ;;
      esac
    done < <(echo "$ln" | grep -oE 'WEB-[0-9]+' || true)
  done < "$BOARD"
  [ "$donec" -gt 5 ] && fail "Board Done备查$donec 条超窗(>5)，请删最旧备查行（卡已在 04_ARCHIVED，无损）"
  [ "$ipc" -gt 2 ] && warn "In Progress $ipc 超上限2"
  [ "$revc" -gt 3 ] && warn "Review $revc 超上限3"
fi
staged="$(git diff --cached --name-status 2>/dev/null || true)"
if echo "$staged" | grep -qE '^R[0-9]*[[:space:]]+\.agents/notes/'; then
  echo "$staged" | grep -q '\.agents/tasks/' || fail "ADR改名已暂存但无任务更新暂存"
fi
br="$(git symbolic-ref --short HEAD 2>/dev/null || true)"
if [[ "$br" =~ ^task/(WEB-[0-9]+)- ]]; then
  bid="${BASH_REMATCH[1]}"
  if ! echo " $IDS " | grep -q " $bid "; then warn "当前分支$br 无对应任务卡（幽灵分支）"
  else
    actual="${ST_OF[$bid]:-}"
    case "$actual" in "In Progress"|Review) ;;
      *) warn "分支$br 对应卡状态为$actual，非In Progress/Review";; esac
    reg="${BR_OF[$bid]:-}"
    [ -z "$reg" ] || [ "$reg" = "$br" ] || warn "分支$br 与卡登记($reg)不一致"
  fi
fi
for f in "$TASKS_DIR"/*.md; do
  [ -e "$f" ] || continue
  st="$(sed -n 's/^- Status:[[:space:]]*//p' "$f" | head -n1 | tr -d '\r' | sed 's/[[:space:]]*$//')"
  case "$st" in "In Progress"|Review)
    grep -qE '^- Branch:[[:space:]]*task/\S+' "$f" || warn "$(basename "$f")：$st 缺 - Branch 行" ;;
  esac
done
if git rev-parse --verify --quiet main >/dev/null 2>&1; then
git branch --format='%(refname:short)' --list 'task/WEB-*' 2>/dev/null | while read -r mb; do
  [ -n "$mb" ] || continue
  git show-ref --verify --quiet "refs/heads/$mb" 2>/dev/null || continue
  if [ -z "$(git cherry main "$mb" 2>/dev/null | grep -E '^\+' || true)" ]; then
    printf 'WARN  已合分支未清 %s（cherry 为空即等价已合，含 squash；收口：先worktree remove再branch -D，仅此条件允许 -D）\n' "$mb" >&2
  fi
done
fi
[ "$FAIL" = "0" ] && echo "check-tasks.sh: 全部通过（WARN见上，语义靠review）"
exit "$FAIL"
