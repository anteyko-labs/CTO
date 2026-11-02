#!/usr/bin/env bash
# Проверка deploy.sh (задача 011) в контейнере Ubuntu с заглушками вместо systemd, PostgreSQL и
# сборки. Запуск с Мака: deploy/cloud/test/run.sh
set -uo pipefail
D=/opt/avtodom/scripts/deploy.sh
R=/opt/avtodom/releases
fails=0
ok() { echo "  ок: $*"; }
bad() { echo "  НЕ ТАК: $*"; fails=$((fails + 1)); }
check() { if eval "$2"; then ok "$1"; else bad "$1"; echo "$out" | tail -5 | sed "s/^/      /"; fi; }
mkrel() { # имя, миграции, [fail|hang]
  mkdir -p $R/$1/bin $R/$1/web
  printf '\x7fELF' > $R/$1/bin/avtodom-server; chmod 755 $R/$1/bin/avtodom-server
  echo x > $R/$1/web/index.html; echo "c-$1" > $R/$1/commit.txt; echo "$2" > $R/$1/migrations.txt
  [ -z "${3:-}" ] || touch $R/$1/$3
}
cur() { basename "$(readlink /opt/avtodom/current)"; }
prev() { basename "$(readlink /opt/avtodom/previous 2> /dev/null)" 2> /dev/null; }
svc() { cat /state/svc; }

echo "1 2 3" > /state/db; echo stopped > /state/svc; : > /state/history
mkrel r1 "1 2 3"; mkrel r2 "1 2 3 4"
ln -sfn releases/r1 /opt/avtodom/current; systemctl start avtodom

echo "== обычная выкатка"
$D --activate r2 > /dev/null 2>&1
check "работает r2, прежняя r1, миграция 4 в базе" '[ "$(cur)" = r2 ] && [ "$(prev)" = r1 ] && grep -qw 4 /state/db && [ "$(svc)" = running ]'

echo "== п.1: версия без миграции базы — отказ до остановки"
mkrel r3 "1 2 3"; : > /state/history
out=$($D --activate r3 2>&1); rc=$?
check "отказ" '[ $rc != 0 ] && grep -q "миграции 4" <<< "$out"'
check "сервер не останавливали, r2 работает" '[ ! -s /state/history ] && [ "$(cur)" = r2 ] && [ "$(svc)" = running ]'

echo "== п.2: сервер падает при запуске — откат без долгого ожидания"
mkrel r4 "1 2 3 4" fail
t0=$(date +%s); out=$(DEPLOY_WAIT=300 $D --activate r4 2>&1); rc=$?; dt=$(( $(date +%s) - t0 ))
check "откат за $dt с (< 20)" '[ $rc != 0 ] && [ $dt -lt 20 ] && grep -q "упал при запуске" <<< "$out"'
check "работает r2, прежняя по-прежнему r1" '[ "$(cur)" = r2 ] && [ "$(svc)" = running ] && [ "$(prev)" = r1 ]'

echo "== п.6: после сломанной включаем рабочую — сломанная не становится «прежней»"
ln -sfn releases/r4 /opt/avtodom/current; systemctl start avtodom
$D --activate r2 > /dev/null 2>&1
check "работает r2, прежняя r1 (не r4)" '[ "$(cur)" = r2 ] && [ "$(prev)" = r1 ]'

for sig in TERM HUP; do
  echo "== п.3: выкатку прервали ($sig) — сервер не остаётся лежать"
  mkrel r5$sig "1 2 3 4" hang; : > /var/log/avtodom-deploy.log
  setsid $D --activate r5$sig > /state/out 2>&1 & pid=$!
  for _ in $(seq 50); do [ "$(svc)" = hang ] && break; sleep 0.2; done
  sleep 1; kill -$sig $pid; wait $pid
  check "работает r2" '[ "$(cur)" = r2 ] && [ "$(svc)" = running ]'
  [ $sig != HUP ] || check "ход отката — в журнале выкатки" 'grep -q "прервана (HUP)" /var/log/avtodom-deploy.log'
done

echo "== п.6: откат, который не поможет — отказ"
mkrel r6 "1 2 3 4 6"; $D --activate r6 > /dev/null 2>&1
echo "1 2 3 4" > $R/r6/db-before.txt
echo "== п.6: откат без терминала — отказ, а не тихое ожидание"
out=$(timeout 10 $D --rollback < /dev/null 2>&1); rc=$?
check "отказ с подсказкой --yes" '[ $rc = 1 ] && grep -q -- "--yes" <<< "$out" && [ "$(cur)" = r6 ]'
ln -sfn releases/r3 /opt/avtodom/previous
out=$($D --rollback --yes 2>&1); rc=$?
check "отказ, работает r6" '[ $rc != 0 ] && grep -q "не знает миграций 4" <<< "$out" && [ "$(cur)" = r6 ] && [ "$(svc)" = running ]'
ln -sfn releases/r2 /opt/avtodom/previous
out=$($D --rollback --yes 2>&1); rc=$?
check "обычный откат на r2 с возвратом базы" '[ $rc = 0 ] && [ "$(cur)" = r2 ] && ! grep -qw 6 /state/db && [ "$(svc)" = running ]'

echo "== п.5: занятая блокировка — видно, кто держит"
( exec 9> /run/avtodom-deploy.lock; flock 9; echo "4242 deploy.sh --activate x" > /run/avtodom-deploy.pid; sleep 5 ) & sleep 0.5
out=$($D --activate r1 2>&1); rc=$?
check "отказ с pid" '[ $rc != 0 ] && grep -q 4242 <<< "$out"'
wait

echo "== п.4: только из main"
rm -rf /srv/w /srv/avtodom.git; git init -q -b main /srv/w; cd /srv/w
git config user.email t@t; git config user.name t
mkdir -p server/migrations web; echo '[package]' > server/Cargo.toml
for v in 1 2 3 4; do echo > server/migrations/000${v}_m.sql; done
git add -A; git commit -qm main1; git checkout -qb side; echo > web/x; git add -A; git commit -qm side1
git checkout -q main; git clone -q --bare /srv/w /srv/avtodom.git; chown -R avtodom-build /srv/avtodom.git /var/lib/avtodom-build
git config --system --add safe.directory '*'; cd /
out=$($D side 2>&1); rc=$?
check "ветка не из main — отказ до сборки" '[ $rc != 0 ] && grep -q "не в истории main" <<< "$out" && [ "$(cur)" = r2 ]'
out=$($D side --branch-ok 2>&1); rc=$?
check "с --branch-ok — выкатывается с предупреждением" '[ $rc = 0 ] && grep -q ВНИМАНИЕ <<< "$out" && [ "$(svc)" = running ] && [ "$(cur)" != r2 ]'
out=$($D main 2>&1); rc=$?
check "main — выкатывается" '[ $rc = 0 ] && [ "$(svc)" = running ]'

echo; if [ $fails = 0 ]; then echo "ВСЁ ПРОШЛО"; else echo "ОШИБОК: $fails"; exit 1; fi
