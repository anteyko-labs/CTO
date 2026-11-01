#!/usr/bin/env bash
# Выкатка версии Avtodom на облачном сервере (запуск от root, после setup.sh):
#   deploy.sh <ревизия>              собрать на сервере (git из REPO_URL, cargo с SQLX_OFFLINE=true,
#                                    CARGO_BUILD_JOBS=2, npm) и выкатить; ревизия — коммит, ветка
#                                    или метка;
#   deploy.sh --bundle <архив.tar.gz> выкатить собранное в другом месте: bin/avtodom-server (Linux),
#                                    web/index.html…, migrations.txt (обязательно), commit.txt;
#   deploy.sh --activate <версия>    снова включить уже лежащую версию (имя из --status);
#   deploy.sh --rollback [--yes]     вернуть прежнюю версию, если нужно — вместе с базой;
#   deploy.sh --status [--wait]      что работает, какие версии и копии есть; --wait — ждать ответа
#                                    сервера до 5 минут (миграции при запуске); не ответил — код 1.
# Порядок выкатки: сборка (или архив) в /opt/avtodom/releases/<дата>-<коммит> и проверка версии →
# остановка сервера → копия базы «before-deploy» (не вышла — прежняя версия запускается снова,
# выкатка отменена) → ссылка current на новую версию (атомарно) → запуск → проверка (страница / —
# 200, /api/v1/auth/me — 401) до 5 минут: сервер при запуске применяет миграции. Не ответил —
# сервер останавливается, база возвращается из копии, если миграции её изменили (прежняя
# программа не запускается на базе с чужими миграциями), и запускается прежняя версия.
# Служба выключена (systemctl disable avtodom — работает Мак) — выкатка отказывается.
# --no-systemd — проверка в контейнере: сервер запускается в фоне, а не службой.
set -euo pipefail
# Язык: ssh с Мака передаёт LANG и LC_*, которых на сервере может не быть.
export LC_ALL=C.UTF-8 LANG=C.UTF-8
unset LANGUAGE

ROOT=/opt/avtodom
REL=$ROOT/releases
SCRIPTS=$ROOT/scripts
BUILD_HOME=/var/lib/avtodom-build
DB=avtodom
WAIT=${DEPLOY_WAIT:-300}
KEEP_RELEASES=5

systemd=1
yes=0
wait=0
args=()
for a in "$@"; do
  case "$a" in
    --no-systemd) systemd=0 ;;
    --yes) yes=1 ;;
    --wait) wait=1 ;;
    *) args+=("$a") ;;
  esac
done
set -- "${args[@]+"${args[@]}"}"

die() { echo "ОШИБКА: $*" >&2; exit 1; }
say() { printf '== %s\n' "$*"; }
[ "$(id -u)" = 0 ] || die "запускать от root"
cd /
[ -f /etc/avtodom/deploy.conf ] || die "нет /etc/avtodom/deploy.conf — сначала setup.sh"
# shellcheck source=/dev/null
. /etc/avtodom/deploy.conf
if [ "${1:-}" != --status ]; then
  exec 9> /run/avtodom-deploy.lock
  flock -n 9 || die "уже идёт другая выкатка"
fi
BUILD_LOG=$BUILD_HOME/build.log

as_postgres() { runuser -u postgres -- "$@"; }
as_build() {
  runuser -u avtodom-build -- env HOME="$BUILD_HOME" \
    PATH="$BUILD_HOME/.cargo/bin:/opt/node/bin:/usr/local/bin:/usr/bin:/bin" "$@"
}

# --- служба ---------------------------------------------------------------------------------
port_busy() { curl -s -o /dev/null --max-time 2 http://127.0.0.1:8080/; }
svc_stop() {
  if [ "$systemd" = 1 ]; then
    systemctl stop avtodom
  else
    pkill -INT -f "^$ROOT/bin/avtodom-server" || true
    for _ in $(seq 30); do port_busy || return 0; sleep 1; done
    pkill -KILL -f "^$ROOT/bin/avtodom-server" || true
  fi
}
svc_start() {
  if [ "$systemd" = 1 ]; then
    systemctl start avtodom
  else
    # Настройки — через окружение, не в командной строке (в ps не виден пароль базы).
    (
      set -a
      # shellcheck source=/dev/null
      . /etc/avtodom/env
      set +a
      export BIND_ADDR=127.0.0.1:8080 WEB_DIR="$ROOT/web" TZ=Asia/Bishkek
      exec runuser -u avtodom -- setsid "$ROOT/bin/avtodom-server" >> /var/lib/avtodom/server.log 2>&1 < /dev/null 9>&-
    ) &
  fi
}
svc_log() {
  if [ "$systemd" = 1 ]; then
    journalctl -u avtodom -n 40 --no-pager >&2 || true
  else
    tail -n 40 /var/lib/avtodom/server.log >&2 || true
  fi
}
healthy() {
  local i root me
  for i in $(seq "$WAIT"); do
    root=$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 http://127.0.0.1:8080/ || true)
    me=$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 http://127.0.0.1:8080/api/v1/auth/me || true)
    if [ "$root" = 200 ] && [ "$me" = 401 ]; then return 0; fi
    [ $((i % 30)) != 0 ] || echo "ждём сервер: $i с (миграции базы идут при запуске)"
    sleep 1
  done
  return 1
}

# --- версии и база ---------------------------------------------------------------------------
current() { if [ -L "$ROOT/current" ]; then basename "$(readlink "$ROOT/current")"; fi; }
previous() { if [ -L "$ROOT/previous" ]; then basename "$(readlink "$ROOT/previous")"; fi; }
# Ссылка меняется атомарно: новая рядом и rename поверх старой.
point() {
  ln -sfn "releases/$2" "$ROOT/.$1.new"
  mv -T "$ROOT/.$1.new" "$ROOT/$1"
}
db_has_data() {
  [ "$(as_postgres psql -X -At -d "$DB" -c "select to_regclass('public.sales') is not null" 2> /dev/null)" = t ]
}
# Применённые миграции базы: номера через пробел.
db_migrations() {
  if [ "$(as_postgres psql -X -At -d "$DB" -c "select to_regclass('public._sqlx_migrations') is not null")" = t ]; then
    as_postgres psql -X -At -d "$DB" -c "select coalesce(string_agg(version::text, ' ' order by version), '') from _sqlx_migrations where success"
  fi
}
# Есть ли в базе миграции, которых не знает версия $1 (тогда она на этой базе не запустится).
db_ahead_of() {
  local list=$REL/$1/migrations.txt v
  [ -f "$list" ] || return 0
  for v in $(db_migrations); do
    grep -qw -- "$v" "$list" || return 0
  done
  return 1
}
commit_of() { cat "$REL/$1/commit.txt" 2> /dev/null || echo '?'; }

check_release() {
  local d=$REL/$1
  [ -x "$d/bin/avtodom-server" ] || die "в версии $1 нет bin/avtodom-server"
  [ "$(head -c 4 "$d/bin/avtodom-server" | od -An -tx1 | tr -d ' \n')" = 7f454c46 ] \
    || die "bin/avtodom-server в версии $1 — не программа Linux"
  [ -f "$d/web/index.html" ] || die "в версии $1 нет web/index.html"
}

prune() {
  local keep_cur keep_prev d n=0
  keep_cur=$(current)
  keep_prev=$(previous)
  while read -r d; do
    n=$((n + 1))
    if [ "$n" -gt "$KEEP_RELEASES" ] && [ "$d" != "$keep_cur" ] && [ "$d" != "$keep_prev" ]; then
      rm -rf -- "${REL:?}/$d"
    fi
  done < <(find "$REL" -mindepth 1 -maxdepth 1 -type d ! -name '.*' -printf '%f\n' | sort -r)
}

# Выключенная служба — сервер работает на Маке (README 2.9): бот и банк — только в одном месте.
require_enabled() {
  if [ "$systemd" = 1 ] && [ "$(systemctl is-enabled avtodom 2> /dev/null || true)" != enabled ]; then
    die "служба avtodom выключена (systemctl disable — сервер работает на Маке?). Снова здесь: systemctl enable avtodom"
  fi
}

# --- выкатка ---------------------------------------------------------------------------------
activate() {
  local new=$1 cur dump='' mig_before mig_after
  check_release "$new"
  cur=$(current)
  [ "$new" != "$cur" ] || die "версия $new уже работает"
  require_enabled
  say "Включаю $new (коммит $(commit_of "$new")), было: ${cur:-ничего}"
  # Сначала остановка, потом копия: после копии в базу не попадёт ни один чек.
  if [ -n "$cur" ]; then
    say "Останавливаю сервер"
    svc_stop
  fi
  if db_has_data; then
    say "Копия базы перед выкаткой"
    if ! dump=$("$SCRIPTS/backup.sh" "before-deploy-$(commit_of "$new" | tr -cd 'A-Za-z0-9' | cut -c1-12)" | tail -n 1); then
      if [ -n "$cur" ]; then
        svc_start
        healthy || svc_log
      fi
      die "копия базы не создана — выкатка отменена, работает прежняя версия ${cur:-(нет)}"
    fi
    echo "$dump" > "$REL/$new/dump-before.txt"
  fi
  mig_before=$(db_migrations)
  point current "$new"
  svc_start
  if healthy; then
    [ -z "$cur" ] || point previous "$cur"
    prune
    say "Готово: работает $new. Откат: deploy.sh --rollback"
    return 0
  fi
  echo "Новая версия не ответила за $WAIT с. Журнал:" >&2
  svc_log
  svc_stop
  if [ -z "$cur" ]; then
    die "первая версия не запустилась, сервер остановлен (см. журнал выше)"
  fi
  mig_after=$(db_migrations)
  if [ "$mig_after" != "$mig_before" ]; then
    say "Миграции изменили базу — возвращаю её из $dump"
    [ -n "$dump" ] || die "копии перед выкаткой нет — сервер остановлен, база новой версии"
    "$SCRIPTS/restore.sh" "$dump" --yes --no-start \
      || die "база не восстановилась — сервер остановлен; команда возврата — выше"
  fi
  point current "$cur"
  svc_start
  if healthy; then
    die "выкатка $new не удалась — работает прежняя версия $cur"
  fi
  svc_log
  die "выкатка не удалась, и прежняя версия $cur тоже не отвечает — смотреть журнал"
}

stage() {
  local name=$1
  rm -rf "${REL:?}/.$name"
  install -d -m 755 "$REL/.$name/bin" "$REL/.$name/web"
  echo "$REL/.$name"
}
publish() {
  chown -R root:root "$REL/.$1"
  chmod -R go-w "$REL/.$1"
  mv -T "$REL/.$1" "$REL/$1"
}

build_rev() {
  local rev=$1 src=$BUILD_HOME/src commit name dir
  if [ ! -x "$BUILD_HOME/.cargo/bin/cargo" ] || [ ! -x /opt/node/bin/npm ]; then
    die "нет Rust или Node — setup.sh без --no-build-tools или deploy.sh --bundle"
  fi
  say "Код: $REPO_URL, ревизия $rev"
  [ -d "$src/.git" ] || as_build git clone -q "$REPO_URL" "$src"
  as_build git -C "$src" remote set-url origin "$REPO_URL"
  as_build git -C "$src" fetch -q --force --tags origin '+refs/heads/*:refs/remotes/origin/*'
  commit=$(as_build git -C "$src" rev-parse -q --verify "origin/$rev^{commit}" \
    || as_build git -C "$src" rev-parse -q --verify "$rev^{commit}") || die "нет ревизии $rev в $REPO_URL"
  as_build git -C "$src" checkout -q --force --detach "$commit"
  as_build git -C "$src" clean -q -fdx -e server/target
  say "Сборка сервера (несколько минут; журнал — $BUILD_LOG)"
  : > "$BUILD_LOG"
  chown avtodom-build "$BUILD_LOG"
  as_build env SQLX_OFFLINE=true CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}" CARGO_TERM_COLOR=never \
    cargo build --release --locked --manifest-path "$src/server/Cargo.toml" >> "$BUILD_LOG" 2>&1 \
    || { tail -n 40 "$BUILD_LOG" >&2; die "сервер не собрался — ничего не изменено"; }
  say "Сборка клиента"
  { as_build npm --prefix "$src/web" ci --no-audit --no-fund && as_build npm --prefix "$src/web" run build; } \
    >> "$BUILD_LOG" 2>&1 || { tail -n 40 "$BUILD_LOG" >&2; die "клиент не собрался — ничего не изменено"; }
  name="$(date +%Y%m%d-%H%M%S)-${commit:0:12}"
  dir=$(stage "$name")
  cp "$src/server/target/release/avtodom-server" "$dir/bin/"
  cp -R "$src/web/dist/." "$dir/web/"
  echo "$commit" > "$dir/commit.txt"
  find "$src/server/migrations" -maxdepth 1 -name '*.sql' -printf '%f\n' \
    | sed -nE 's/^0*([0-9]+)_.*/\1/p' | sort -n | tr '\n' ' ' > "$dir/migrations.txt"
  check_release ".$name"
  publish "$name"
  activate "$name"
}

from_bundle() {
  local file=$1 tmp name dir commit
  [ -f "$file" ] || die "нет файла $file"
  tmp=$(mktemp -d)
  tar -xzf "$file" -C "$tmp" || { rm -rf "$tmp"; die "архив $file не распаковался"; }
  # Список миграций обязателен: без него откат не знает, подходит ли база прежней версии.
  if [ ! -f "$tmp/migrations.txt" ] || ! grep -qE '^[0-9 ]*[0-9][0-9 ]*$' "$tmp/migrations.txt"; then
    rm -rf "$tmp"
    die "в архиве нет migrations.txt (номера миграций через пробел, README 2.8) — ничего не изменено"
  fi
  commit=$(head -n 1 "$tmp/commit.txt" 2> /dev/null | tr -cd 'A-Za-z0-9')
  commit=${commit:-bundle}
  name="$(date +%Y%m%d-%H%M%S)-${commit:0:12}"
  dir=$(stage "$name")
  if [ -f "$tmp/bin/avtodom-server" ]; then cp "$tmp/bin/avtodom-server" "$dir/bin/"; else cp "$tmp/avtodom-server" "$dir/bin/" 2> /dev/null || true; fi
  chmod 755 "$dir/bin/avtodom-server" 2> /dev/null || true
  [ ! -d "$tmp/web" ] || cp -R "$tmp/web/." "$dir/web/"
  echo "$commit" > "$dir/commit.txt"
  cp "$tmp/migrations.txt" "$dir/"
  rm -rf "$tmp"
  check_release ".$name"
  publish "$name"
  activate "$name"
}

rollback() {
  local cur prev dump='' restore=0 answer
  cur=$(current)
  prev=$(previous)
  if [ -z "$prev" ] || [ ! -d "$REL/$prev" ]; then die "прежней версии нет — откатывать не к чему"; fi
  [ "$prev" != "$cur" ] || die "откат уже выполнен"
  check_release "$prev"
  require_enabled
  [ ! -f "$REL/$cur/dump-before.txt" ] || dump=$(cat "$REL/$cur/dump-before.txt")
  if db_ahead_of "$prev"; then restore=1; fi
  echo "Сейчас:   $cur (коммит $(commit_of "$cur"))"
  echo "Вернётся: $prev (коммит $(commit_of "$prev"))"
  if [ "$restore" = 1 ]; then
    if [ -z "$dump" ] || [ ! -f "$dump" ]; then
      die "прежняя версия не знает миграций базы, а копии перед выкаткой нет"
    fi
    echo "База вернётся из копии перед выкаткой: $dump"
    echo "Всё, что внесено после выкатки, пропадёт (копия текущей базы сохранится)."
  else
    echo "База не меняется: прежняя версия работает с ней как есть."
  fi
  if [ "$yes" != 1 ]; then
    read -r -p 'Чтобы откатить, введите ОТКАТ: ' answer
    [ "$answer" = 'ОТКАТ' ] || { echo 'Отменено, ничего не изменено.'; exit 1; }
  fi
  svc_stop
  if [ "$restore" = 1 ] && ! "$SCRIPTS/restore.sh" "$dump" --yes --no-start; then
    svc_start || true
    die "откат не выполнен: база не восстановилась, программа не менялась"
  fi
  point current "$prev"
  rm -f "$ROOT/previous"
  svc_start
  healthy || { svc_log; die "прежняя версия не отвечает"; }
  say "Откат выполнен: работает $prev. Вернуть новую: deploy.sh --activate $cur"
}

status() {
  local cur prev d mark ok=0
  cur=$(current)
  prev=$(previous)
  if [ "$systemd" = 1 ]; then
    echo "Служба: $(systemctl is-active avtodom || true), автозапуск: $(systemctl is-enabled avtodom 2> /dev/null || true)"
  fi
  if [ "$wait" = 1 ]; then
    if healthy; then ok=1; fi
  elif healthy_once; then
    ok=1
  fi
  if [ "$ok" = 1 ]; then echo "Сервер отвечает: да"; else echo "Сервер отвечает: НЕТ"; fi
  echo "Версии (новые сверху):"
  while read -r d; do
    mark='  '
    [ "$d" != "$cur" ] || mark='* '
    [ "$d" != "$prev" ] || mark='< '
    echo "  $mark$d  коммит $(commit_of "$d")"
  done < <(find "$REL" -mindepth 1 -maxdepth 1 -type d ! -name '.*' -printf '%f\n' | sort -r)
  echo "  (* — работает, < — прежняя для отката)"
  echo "Последние копии базы:"
  find /var/backups/avtodom -maxdepth 1 -name 'avtodom-*.dump' -printf '  %TY-%Tm-%Td %TH:%TM  %s байт  %f\n' | sort -r | head -n 5
  if [ "$ok" != 1 ] && [ "$wait" = 1 ]; then
    echo "Сервер не ответил за $WAIT с. Журнал:" >&2
    svc_log
  fi
  [ "$ok" = 1 ]
}
healthy_once() { WAIT=1 healthy > /dev/null; }

case "${1:-}" in
  --status) status ;;
  --rollback) rollback ;;
  --activate) [ -n "${2:-}" ] || die "deploy.sh --activate <версия>"; activate "$2" ;;
  --bundle) [ -n "${2:-}" ] || die "deploy.sh --bundle <архив.tar.gz>"; from_bundle "$2" ;;
  -* | '') sed -n '2,19p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
  *) build_rev "$1" ;;
esac
