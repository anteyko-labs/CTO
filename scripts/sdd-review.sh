#!/bin/bash
# Проверки после задачи (Spec-as-Source). Печатает только итог и ошибки.
# Использование: scripts/sdd-review.sh [server|web|all]   (по умолчанию all)
# Тесты сервера идут на базе avtodom_e2e_review (рабочую avtodom не трогают): её адрес —
# DATABASE_URL из server/.env с заменённым именем базы; можно задать REVIEW_DATABASE_URL.
set -u
root=$(git rev-parse --show-toplevel 2>/dev/null) || { echo "не git-репозиторий" >&2; exit 1; }
what=${1:-all}
fail=0

step() {
  local name=$1; shift
  local out
  if out=$("$@" 2>&1); then
    echo "ok   $name"
  else
    echo "FAIL $name"
    printf '%s\n' "$out" | grep -E 'error|Error|FAIL|failed|panicked|✗|×' | head -n 30
    fail=1
  fi
}

if [ "$what" = server ] || [ "$what" = all ]; then
  [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
  cd "$root/server" || exit 1
  base=${REVIEW_DATABASE_URL:-$(grep -E '^DATABASE_URL=' .env 2>/dev/null | cut -d= -f2- | sed -E 's#/[^/]+$#/avtodom_e2e_review#')}
  if [ -z "$base" ]; then echo "FAIL нет DATABASE_URL в server/.env" >&2; exit 1; fi
  case "$base" in */avtodom) echo "FAIL отказ: рабочая база avtodom" >&2; exit 1 ;; esac
  export DATABASE_URL=$base
  db=${base##*/}
  dropdb -U postgres --if-exists "$db" >/dev/null 2>&1
  createdb -U postgres "$db" >/dev/null 2>&1
  step "миграции ($db)" sqlx migrate run
  step "cargo fmt" cargo fmt --all -- --check
  step "cargo clippy" cargo clippy --all-targets -- -D warnings
  step "sqlx prepare --check" cargo sqlx prepare --check -- --all-targets
  step "cargo test" cargo test --no-fail-fast
fi

if [ "$what" = web ] || [ "$what" = all ]; then
  cd "$root/web" || exit 1
  step "tsc" npx tsc -b
  step "oxlint" npx oxlint src
  step "npm test" npm test
  step "vite build" npx vite build
fi

# Полный e2e — только перед выкатом (своя база avtodom_e2e*, свой порт, env как в ci.yml):
#   cd web && CI=true E2E_BASE_URL=http://127.0.0.1:<порт> E2E_LOGIN=owner E2E_PASSWORD=owner-pass-1 npx playwright test

[ $fail -eq 0 ] && echo "ИТОГ: всё прошло" || echo "ИТОГ: есть ошибки"
exit $fail
