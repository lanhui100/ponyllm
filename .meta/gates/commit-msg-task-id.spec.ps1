#!/usr/bin/env bash
# 负样本测试：commit-msg-task-id.spec.sh
set -u

# 使用 shell 函数覆盖 git 命令来模拟 git 环境
git() {
  if [ "$1" == "symbolic-ref" ]; then
    echo "task/WEB-123"
  else
    command git "$@"
  fi
}
export -f git

COMMIT_MSG="fix: something unrelated"
echo "$COMMIT_MSG" > .git/hooks/test-msg

echo "Running commit-msg-task-id.sh against invalid commit message..."
.meta/gates/commit-msg-task-id.sh .git/hooks/test-msg
RET=$?

# 清理
rm -f .git/hooks/test-msg

if [ $RET -ne 0 ]; then
  echo "Spec passed: commit-msg-task-id correctly identified invalid message."
  exit 0
else
  echo "Spec failed: commit-msg-task-id failed to identify invalid message."
  exit 1
fi
