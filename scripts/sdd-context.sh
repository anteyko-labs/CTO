#!/bin/bash
# Контекст для начала работы (Spec-as-Source): инварианты, контракты, карта и, если указана,
# одна задача. Использование: scripts/sdd-context.sh [NNN]  — NNN номер файла в docs/sdd/tasks/.
set -u
root=$(git rev-parse --show-toplevel 2>/dev/null) || { echo "не git-репозиторий" >&2; exit 1; }
cd "$root" || exit 1

show() {
  [ -f "$1" ] || { echo "!! нет $1" >&2; return; }
  printf '\n===== %s =====\n' "$1"
  cat "$1"
}

show docs/sdd/digest.md
show docs/sdd/tier0-invariants.md
show docs/sdd/tier2-contracts.md

printf '\n===== задачи =====\n'
for f in docs/sdd/tasks/*.md; do
  [ -f "$f" ] || continue
  status=$(grep -m1 -iE '^(статус|status):' "$f" | sed -E 's/^[^:]*:[[:space:]]*//')
  printf '%s  %s\n' "$(basename "$f")" "${status:-—}"
done

if [ $# -ge 1 ]; then
  task=$(ls docs/sdd/tasks/"$1"-*.md 2>/dev/null | head -n1)
  [ -n "$task" ] && show "$task" || echo "!! задачи $1 нет" >&2
fi

printf '\n===== git =====\n'
git log --oneline -3
git status --short | head -n 15
