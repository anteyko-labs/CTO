#!/bin/sh
# Общие функции хуков.

# Локальный список запрещённых слов (по одному регулярному выражению в строке).
FORBIDDEN_FILE="$(git rev-parse --show-toplevel)/.githooks/forbidden.local"

# Проверяет stdin на запрещённые слова. Печатает совпадения, возвращает 1 при находке.
check_forbidden() {
  [ -f "$FORBIDDEN_FILE" ] || return 0
  matches=$(grep -inE -f "$FORBIDDEN_FILE")
  [ -z "$matches" ] && return 0
  printf '%s\n' "$matches"
  return 1
}
