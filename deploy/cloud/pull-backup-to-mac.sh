#!/bin/bash
# Мак точки забирает копии базы из облака (вторая копия вне облака). Раз в день его запускает
# launchd (kg.avtodom.pull-backup.plist), можно и руками:
#   AVTODOM_CLOUD_HOST=avtodom.kg deploy/cloud/pull-backup-to-mac.sh
# Ключ — ~/.ssh/avtodom_pull (ssh-keygen -t ed25519 -f ~/.ssh/avtodom_pull -N ''), его открытая
# часть отдана setup.sh --pull-key-file: этим ключом можно только читать копии (backup-serve.sh).
# Забирает недостающие, сверяет размер и sha256, держит 30 последних в ~/avtodom-backups/cloud.
# Ночной копии за сегодня или вчера нет — код 1 и строка «ВНИМАНИЕ» в журнале (pull.log).
# Написан под /bin/bash 3.2 macOS.
set -euo pipefail

HOST=${AVTODOM_CLOUD_HOST:?задайте AVTODOM_CLOUD_HOST — домен или адрес облачного сервера}
KEY=${AVTODOM_PULL_KEY:-$HOME/.ssh/avtodom_pull}
DEST=${AVTODOM_PULL_DEST:-$HOME/avtodom-backups/cloud}
KEEP=${AVTODOM_PULL_KEEP:-30}
PG_RESTORE=${PG_RESTORE:-/opt/homebrew/opt/postgresql@16/bin/pg_restore}

log() { echo "$(date '+%Y-%m-%d %H:%M:%S') $*"; }
# stdin — /dev/null: иначе ssh внутри цикла `while read` съест список копий.
remote() {
  ssh -n -i "$KEY" -o BatchMode=yes -o IdentitiesOnly=yes -o StrictHostKeyChecking=accept-new \
    -o ConnectTimeout=20 -o ServerAliveInterval=30 "avtodom-pull@$HOST" "$@" < /dev/null
}

mkdir -p "$DEST"
list=$(remote list) || { log "ОШИБКА: облачный сервер $HOST не ответил"; exit 1; }
# Хранятся KEEP последних по имени (в имени — дата) из копий облака и уже забранных: старше —
# не скачиваются и удаляются, иначе удалённое здесь скачивалось бы снова каждый день.
keep=$({ printf '%s\n' "$list" | cut -d' ' -f1; find "$DEST" -maxdepth 1 -type f -name 'avtodom-*.dump' -exec basename {} \;; } \
  | grep -E '^avtodom-[A-Za-z0-9._-]+\.dump$' | sort -ru | awk -v k="$KEEP" 'NR <= k')  || keep=''
kept() { printf '%s\n' "$keep" | grep -xF -- "$1" > /dev/null; }
got=0
while read -r name size; do
  [ -n "${name:-}" ] || continue
  case "$name" in avtodom-*.dump) ;; *) continue ;; esac
  kept "$name" || continue
  if [ -f "$DEST/$name" ] && [ "$(stat -f %z "$DEST/$name")" = "$size" ]; then continue; fi
  part="$DEST/.$name.part"
  remote get "$name" > "$part" || { rm -f "$part"; log "ОШИБКА: $name не скачан"; exit 1; }
  if [ "$(stat -f %z "$part")" != "$size" ]; then rm -f "$part"; log "ОШИБКА: $name — не тот размер"; exit 1; fi
  if [ "$(remote sum "$name")" != "$(shasum -a 256 "$part" | cut -d' ' -f1)" ]; then
    rm -f "$part"; log "ОШИБКА: $name — не та контрольная сумма"; exit 1
  fi
  if [ -x "$PG_RESTORE" ]; then
    # Список — целиком в переменную: grep -q с pipefail обрывал бы pg_restore.
    toc=$("$PG_RESTORE" --list "$part" 2> /dev/null) || toc=''
    if ! printf '%s\n' "$toc" | grep -E 'TABLE DATA public sales( |$)' > /dev/null; then
      rm -f "$part"; log "ОШИБКА: $name не читается как копия базы"; exit 1
    fi
  fi
  mv -f "$part" "$DEST/$name"
  got=$((got + 1))
  log "забрана $name ($size байт)"
done <<< "$list"

while read -r f; do
  [ -z "$f" ] || kept "$f" || rm -f -- "${DEST:?}/$f"
done <<< "$(find "$DEST" -maxdepth 1 -type f -name 'avtodom-*.dump' -exec basename {} \;)"

today=$(date +%F)
yesterday=$(date -v-1d +%F)
if [ -f "$DEST/avtodom-$today.dump" ] || [ -f "$DEST/avtodom-$yesterday.dump" ]; then
  log "готово: новых $got, свежая копия есть"
else
  log "ВНИМАНИЕ: в облаке нет ночной копии за $yesterday и $today — проверить сервер (journalctl -u avtodom-backup)"
  exit 1
fi
