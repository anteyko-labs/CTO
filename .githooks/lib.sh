#!/bin/sh
# Общие функции хуков.

# Локальный список запрещённых слов (по одному регулярному выражению в строке).
FORBIDDEN_FILE="$(git rev-parse --show-toplevel)/.githooks/forbidden.local"

# Без списка проверка невозможна — коммит останавливается, а не проходит молча.
require_forbidden_list() {
  [ -f "$FORBIDDEN_FILE" ] && return 0
  echo "нет .githooks/forbidden.local: без списка запрещённых слов коммит не проходит." >&2
  echo "Скопируйте файл с рабочей машины (через git он не ездит)." >&2
  return 1
}

# Проверяет stdin на запрещённые слова. Печатает совпадения, возвращает 1 при находке
# и при отсутствии списка.
check_forbidden() {
  [ -f "$FORBIDDEN_FILE" ] || return 1
  matches=$(grep -inE -f "$FORBIDDEN_FILE")
  [ -z "$matches" ] && return 0
  printf '%s\n' "$matches"
  return 1
}
