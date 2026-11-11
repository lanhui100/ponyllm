#!/usr/bin/env bash
# 负样本测试：check-tasks.spec.sh
# 构造一个损坏的任务文件，测试 check-tasks.sh 是否能正确报错。
set -u
ROOT_DIR="$(pwd)"
TEST_DIR="$(mktemp -d)"
# 模拟项目结构
mkdir -p "$TEST_DIR/.agents/tasks/web-console/03_TASKS"
TASKS_DIR="$TEST_DIR/.agents/tasks/web-console/03_TASKS"
# 创建一个缺失节的任务文件
TASK_FILE="$TASKS_DIR/WEB-123-bad.md"
cat <<EOF > "$TASK_FILE"
- ID: WEB-123
- Status: Ready
- Priority: P1
- Created At: 2026-01-01
- Updated At: 2026-01-01
- Branch: task/WEB-123-test

## Basic Info
EOF

# 运行 check-tasks.sh 并预期失败
# 注意：check-tasks.sh 内部依赖 $ROOT_DIR/.agents/tasks/web-console...
# 我们需要将测试环境模拟成一个真正的项目根目录
# 为了简化，直接在项目内创建一个临时的损坏文件进行测试，运行后删除

BAD_TASK=".agents/tasks/web-console/03_TASKS/WEB-999-spec.md"
mkdir -p "$(dirname "$BAD_TASK")"
cat <<EOF > "$BAD_TASK"
- ID: WEB-999
- Status: Ready
- Priority: P1
- Created At: 2026-01-01
- Updated At: 2026-01-01
- Branch: task/WEB-999-test

## Basic Info
EOF

echo "Running check-tasks.sh against corrupted task..."
.meta/gates/check-tasks.sh
RET=$?

# 清理
rm -f "$BAD_TASK"

if [ $RET -ne 0 ]; then
  echo "Spec passed: check-tasks correctly identified corrupted task."
  exit 0
else
  echo "Spec failed: check-tasks failed to identify corrupted task."
  exit 1
fi
