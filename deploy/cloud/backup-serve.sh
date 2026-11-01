#!/usr/bin/env bash
# Выдача копий базы Маку точки. Стоит в authorized_keys пользователя avtodom-pull
# (command="/opt/avtodom/scripts/backup-serve.sh",restrict): с этим ключом можно только читать
# копии из /var/backups/avtodom — ни входа в оболочку, ни записи. Команды (SSH_ORIGINAL_COMMAND):
#   list          — имена и размеры копий;
#   sum <файл>    — sha256 копии;
#   get <файл>    — сама копия в stdout.
set -euo pipefail

DIR=/var/backups/avtodom
read -r cmd name _ <<< "${SSH_ORIGINAL_COMMAND:-}" || true

valid() {
  if [[ "${name:-}" =~ ^avtodom-[A-Za-z0-9._-]+\.dump$ ]] && [ -f "$DIR/$name" ]; then return 0; fi
  echo "нет такой копии: ${name:-}" >&2
  exit 1
}

case "${cmd:-}" in
  list) find "$DIR" -maxdepth 1 -type f -name 'avtodom-*.dump' -printf '%f %s\n' | sort ;;
  sum) valid; sha256sum "$DIR/$name" | cut -d' ' -f1 ;;
  get) valid; cat "$DIR/$name" ;;
  *) echo "команды: list | sum <файл> | get <файл>" >&2; exit 2 ;;
esac
