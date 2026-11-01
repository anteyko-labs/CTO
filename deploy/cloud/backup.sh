#!/usr/bin/env bash
# Копия базы Avtodom в облаке: /var/backups/avtodom. Запускается от root.
#   backup.sh                  — ночная копия avtodom-ГГГГ-ММ-ДД.dump (таймер avtodom-backup.timer);
#   backup.sh before-deploy-X  — копия перед выкаткой/восстановлением
#                                avtodom-ГГГГ-ММ-ДД-before-deploy-X-ЧЧММ.dump; путь печатается
#                                последней строкой.
# Копия годна, только если pg_restore --list читает файл и в нём есть данные таблицы продаж;
# негодная не заменяет прежнюю и старые не удаляются. Хранение: 14 последних ночных; копии
# «before-…» — 30 дней, две последние — всегда. Ошибка — в журнал (journalctl -u avtodom-backup)
# и владельцу событием backup.failed («Уведомления» и Телеграм), как на точке (deploy/windows).
# Мак точки забирает копии к себе: deploy/cloud/pull-backup-to-mac.sh.
set -euo pipefail
# Программы PostgreSQL от имени postgres не могут войти в каталог root.
cd /

DB=${AVTODOM_DB:-avtodom}
DIR=${AVTODOM_BACKUPS:-/var/backups/avtodom}
KEEP_DAILY=14
KEEP_BEFORE_DAYS=30
SIZE_WARN_BYTES=$((10 * 1024 * 1024 * 1024))
label=${1:-}

as_postgres() { runuser -u postgres -- "$@"; }

# Запись в журнал действий сервера: владелец увидит её в «Уведомлениях» и в Телеграме. Если не
# работает сама база, запись не пройдёт — остаётся только журнал systemd.
notify() {
  local action=$1 reason=$2
  if ! as_postgres psql -X -q -v ON_ERROR_STOP=1 -d "$DB" -v reason="$reason" -v action="$action" <<'SQL'; then
insert into audit_log (id, branch_id, action, entity, data)
select gen_random_uuid(), id, :'action', 'backup', jsonb_build_object('reason', :'reason')
from branches;
SQL
    echo "ОШИБКА: не удалось сообщить владельцу ($action)" >&2
  fi
}

fail() {
  echo "ОШИБКА: копия не создана — $1" >&2
  notify backup.failed "$1 (облако)"
  exit 1
}

case "$label" in
  '') name="avtodom-$(date +%F).dump" ;;
  before-*)
    [[ "$label" =~ ^before-[A-Za-z0-9._-]+$ ]] || { echo "Метка копии — латиница, цифры, . _ -" >&2; exit 2; }
    name="avtodom-$(date +%F)-$label-$(date +%H%M).dump" ;;
  *) echo "Использование: backup.sh [before-<метка>]" >&2; exit 2 ;;
esac

install -d -m 750 -o root -g avtodom "$DIR"
tmp=$(mktemp "$DIR/.tmp-XXXXXX.dump")
trap 'rm -f "$tmp"' EXIT

if ! as_postgres pg_dump -Fc -d "$DB" > "$tmp" 2> "$tmp.err"; then
  reason="pg_dump: $(head -c 300 "$tmp.err" | tr '\n' ' ')"
  rm -f "$tmp.err"
  fail "$reason"
fi
rm -f "$tmp.err"
if ! list=$(pg_restore --list "$tmp" 2>&1); then
  fail "файл копии не читается (pg_restore --list)"
fi
if ! grep -qE 'TABLE DATA public sales( |$)' <<< "$list"; then
  fail "в копии нет данных продаж"
fi
chown root:avtodom "$tmp"
chmod 640 "$tmp"
mv -f "$tmp" "$DIR/$name"
trap - EXIT
echo "Копия: $DIR/$name ($(du -h "$DIR/$name" | cut -f1))" >&2

# Хранение — только после удачной копии.
mapfile -t daily < <(find "$DIR" -maxdepth 1 -type f -regex '.*/avtodom-[0-9-]*\.dump' -printf '%f\n' | sort -r)
for old in "${daily[@]:KEEP_DAILY}"; do rm -f -- "${DIR:?}/$old"; done
mapfile -t before < <(find "$DIR" -maxdepth 1 -type f -name 'avtodom-*-before-*.dump' -printf '%T@ %f\n' | sort -rn | cut -d' ' -f2-)
for old in "${before[@]:2}"; do
  if [ -n "$(find "$DIR" -maxdepth 1 -name "$old" -mtime +"$KEEP_BEFORE_DAYS")" ]; then
    rm -f -- "${DIR:?}/$old"
  fi
done

total=$(du -sb "$DIR" | cut -f1)
if [ "$total" -gt "$SIZE_WARN_BYTES" ]; then
  notify backup.size "копии в облаке занимают $((total / 1024 / 1024 / 1024)) ГБ — проверить место на сервере"
fi
printf '%s\n' "$DIR/$name"
