#!/usr/bin/env bash
# Восстановление базы Avtodom в облаке из копии pg_dump -Fc (копия с Мака при переезде, ночная
# копия, копия перед выкаткой). Запуск от root:
#   restore.sh /var/backups/avtodom/avtodom-ГГГГ-ММ-ДД.dump [--yes] [--no-start]
# Порядок — как restore.ps1 на точке: копия проверяется (читается, есть продажи) → вопрос →
# сервер останавливается → копия текущей базы «before-restore» (не вышло — отмена, база не
# тронута) → база пересоздаётся и копия встаёт в чистую (--no-owner, роль avtodom) → сверка
# (verify.sql) → запуск сервера; новая версия сама доведёт базу своими миграциями.
# --no-start — сервер не запускать (переезд: сначала сверить числа с Маком).
set -euo pipefail

DB=avtodom
HERE=$(cd "$(dirname "$0")" && pwd)
dump=''
yes=0
start=1
for a in "$@"; do
  case "$a" in
    --yes) yes=1 ;;
    --no-start) start=0 ;;
    -*) echo "Неизвестный ключ: $a" >&2; exit 2 ;;
    *) dump=$a ;;
  esac
done
[ "$(id -u)" = 0 ] || { echo "Запускать от root" >&2; exit 2; }
if [ -z "$dump" ] || [ ! -f "$dump" ]; then
  echo "Использование: restore.sh <файл.dump> [--yes] [--no-start]" >&2
  exit 2
fi
dump=$(realpath -- "$dump")
# Программы PostgreSQL от имени postgres не могут войти в каталог root.
cd /

as_postgres() { runuser -u postgres -- "$@"; }
has_systemd() { [ -d /run/systemd/system ]; }
svc() { if has_systemd; then systemctl "$@"; fi; }

# Файл читается postgres: копия может лежать в каталоге root (scp в /root).
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cp -- "$dump" "$work/restore.dump"
chown -R postgres "$work"
src="$work/restore.dump"

list=$(pg_restore --list "$src") || { echo "Файл не читается как копия базы: $dump" >&2; exit 1; }
grep -qE 'TABLE DATA public sales( |$)' <<< "$list" || { echo "В копии нет данных продаж: $dump" >&2; exit 1; }

echo "Копия: $dump"
echo "Время файла: $(date -r "$dump" '+%d.%m.%Y %H:%M'), размер $(du -h "$dump" | cut -f1)"
echo "Всё, что внесено в базу после этой копии, пропадёт (копия текущей базы сохранится)."
if [ "$yes" != 1 ]; then
  read -r -p 'Чтобы восстановить, введите ВОССТАНОВИТЬ: ' answer
  [ "$answer" = 'ВОССТАНОВИТЬ' ] || { echo 'Отменено, ничего не изменено.'; exit 1; }
fi

svc stop avtodom || true
exists=$(as_postgres psql -X -At -d postgres -c "select 1 from pg_database where datname = '$DB'")
if [ "$exists" = 1 ] && [ "$(as_postgres psql -X -At -d "$DB" -c "select to_regclass('public.sales') is not null")" = t ]; then
  if ! before=$("$HERE/backup.sh" before-restore | tail -n 1); then
    echo "Копия текущей базы не создана — восстановление отменено, база не тронута." >&2
    svc start avtodom || true
    exit 1
  fi
  echo "Копия текущей базы: $before"
else
  before=''
fi

as_postgres dropdb --force --if-exists "$DB"
as_postgres createdb -O avtodom --template=template0 --locale=en_US.UTF-8 "$DB"
if ! as_postgres pg_restore -d "$DB" --no-owner --no-acl --role=avtodom --exit-on-error "$src"; then
  echo "БАЗА НЕ ВОССТАНОВИЛАСЬ: pg_restore завершился с ошибкой. Сервер не запущен." >&2
  [ -n "$before" ] && echo "Вернуть как было: $HERE/restore.sh '$before' --yes" >&2
  exit 1
fi
as_postgres vacuumdb -q --analyze-only -d "$DB"

echo '--- Сверка (verify.sql) ---'
as_postgres psql -X -At -v ON_ERROR_STOP=1 -d "$DB" < "$HERE/verify.sql" | grep -v '^строк |'
if [ "$start" = 1 ]; then
  svc start avtodom
  echo 'База восстановлена, сервер запущен.'
else
  echo 'База восстановлена, сервер НЕ запущен (--no-start): systemctl start avtodom.'
fi
