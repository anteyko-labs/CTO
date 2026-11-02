#!/usr/bin/env bash
# Заглушки для test-deploy.sh: systemctl, runuser, psql, curl, journalctl, cargo, npm.
# Модель: служба запускает версию current; сервер поднимается, только если все миграции базы
# есть в migrations.txt версии (как sqlx), и тогда дописывает в базу свои. Состояние — в $ST.
set -euo pipefail
ST=/state
B=/usr/local/bin
mkdir -p "$ST" "$B"
cat > $B/runuser <<'S'
#!/usr/bin/env bash
[ "$1" = -u ] && shift 2; [ "$1" = -- ] && shift; exec "$@"
S
cat > $B/systemctl <<'S'
#!/usr/bin/env bash
ST=/state
cmd=$1; shift
case "$cmd" in
  stop) echo stopped > $ST/svc ;;
  start)
    echo 0 > $ST/restarts
    rel=$(readlink -f /opt/avtodom/current); ok=1
    for v in $(cat $ST/db); do grep -qw -- "$v" "$rel/migrations.txt" || ok=0; done
    [ ! -f "$rel/hang" ] || ok=hang; [ ! -f "$rel/fail" ] || ok=0
    if [ "$ok" = 1 ]; then
      echo running > $ST/svc
      echo $(cat $ST/db $rel/migrations.txt | tr ' ' '\n' | grep . | sort -nu) > $ST/db
    elif [ "$ok" = hang ]; then echo hang > $ST/svc
    else echo failing > $ST/svc; date +%s > $ST/failed_at; fi
    echo "$(date +%T) start $(basename $rel) -> $(cat $ST/svc)" >> $ST/history ;;
  show)
    if [ "$(cat $ST/svc)" = failing ]; then
      echo $(( ($(date +%s) - $(cat $ST/failed_at)) / 5 ))
    else cat $ST/restarts; fi ;;
  is-enabled) echo enabled ;;
  is-active) case $(cat $ST/svc) in running) echo active ;; *) echo activating ;; esac ;;
esac
S
cat > $B/journalctl <<'S'
#!/usr/bin/env bash
[ "$(cat /state/svc)" != failing ] || echo "ERROR avtodom_server: миграции: migration N was previously applied"
S
cat > $B/curl <<'S'
#!/usr/bin/env bash
url=${@: -1}
if [ "$(cat /state/svc)" = running ]; then
  case "$url" in */auth/me) printf 401 ;; *) printf 200 ;; esac
else printf 000; exit 7; fi
S
cat > $B/psql <<'S'
#!/usr/bin/env bash
q=${@: -1}
case "$q" in
  *to_regclass*) echo t ;;
  *string_agg*) cat /state/db ;;
esac
S
chmod 755 $B/*
mkdir -p /opt/avtodom/scripts /opt/avtodom/releases /var/backups/avtodom /etc/avtodom /var/lib/avtodom-build/.cargo/bin /opt/node/bin
echo "REPO_URL=/srv/avtodom.git" > /etc/avtodom/deploy.conf
cat > /opt/avtodom/scripts/backup.sh <<'S'
#!/usr/bin/env bash
f=/var/backups/avtodom/avtodom-$(date +%s%N)-$1.dump; cp /state/db "$f"; echo "$f"
S
cat > /opt/avtodom/scripts/restore.sh <<'S'
#!/usr/bin/env bash
cp "$1" /state/db; echo "restore $1" >> /state/history
S
cat > /var/lib/avtodom-build/.cargo/bin/cargo <<'S'
#!/usr/bin/env bash
m=${@: -1}; d=$(dirname "$m")/target/release; mkdir -p "$d"; printf '\x7fELF' > "$d/avtodom-server"; chmod 755 "$d/avtodom-server"
S
cat > /opt/node/bin/npm <<'S'
#!/usr/bin/env bash
[ "$1" = --prefix ] && { mkdir -p "$2/dist"; echo x > "$2/dist/index.html"; }; true
S
chmod 755 /opt/avtodom/scripts/*.sh /var/lib/avtodom-build/.cargo/bin/cargo /opt/node/bin/npm
id avtodom-build >/dev/null 2>&1 || useradd -m avtodom-build
