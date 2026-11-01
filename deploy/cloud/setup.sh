#!/usr/bin/env bash
# Подготовка облачного сервера Avtodom (Ubuntu 24.04, запуск от root). Повторный запуск безопасен:
# пароль базы, /etc/avtodom/env и выкаченные версии сохраняются, файлы настроек переписываются
# из этого каталога; сервер перезапускается, только если изменились его служба или настройки, а
# выключенная служба (systemctl disable avtodom — возврат на Мак) не включается снова.
# Решение — ADR-{cloud.1}, инструкция — deploy/cloud/README.md.
#   bash setup.sh --domain avtodom.kg [--email почта@для.letsencrypt] [--pull-key-file mac.pub]
#                 [--repo URL] [--no-build-tools] [--no-systemd]
# --pull-key-file   открытый ключ Мака точки: ему — только чтение копий базы (backup-serve.sh).
# --repo            откуда deploy.sh берёт код; по умолчанию /srv/avtodom.git (git push с Мака).
# --no-build-tools  не ставить Rust и Node (версии собираются в другом месте: deploy.sh --bundle).
# --no-systemd      проверка в контейнере без systemd: службы не включаются, PostgreSQL
#                   запускается pg_ctlcluster, ufw/swap/sshd не трогаются (конфиги проверяются).
set -euo pipefail
# Язык: ssh с Мака передаёт LANG и LC_* (SendEnv), а на свежем сервере таких локалей нет —
# тогда apt, perl и pg_createcluster падают. C.UTF-8 есть всегда.
export LC_ALL=C.UTF-8 LANG=C.UTF-8
unset LANGUAGE

HERE=$(cd "$(dirname "$0")" && pwd)
cd /
domain=''
email=''
pull_key=''
repo=''
build_tools=1
systemd=1
while [ $# -gt 0 ]; do
  case "$1" in
    --domain) domain=${2:?}; shift 2 ;;
    --email) email=${2:?}; shift 2 ;;
    --pull-key-file) pull_key=$(realpath -- "${2:?}"); shift 2 ;;
    --repo) repo=${2:?}; shift 2 ;;
    --no-build-tools) build_tools=0; shift ;;
    --no-systemd) systemd=0; shift ;;
    *) echo "Неизвестный параметр: $1" >&2; exit 2 ;;
  esac
done

say() { printf '\n== %s\n' "$*"; }
die() { echo "ОШИБКА: $*" >&2; exit 1; }
# Записать stdin в файл, если содержимое другое. Код 0 — файл изменён, 1 — уже такой.
put_file() {
  local dst=$1 mode=$2
  cat > "$dst.new"
  if [ -f "$dst" ] && cmp -s "$dst.new" "$dst"; then
    rm -f "$dst.new"
    return 1
  fi
  chmod "$mode" "$dst.new"
  mv -f "$dst.new" "$dst"
}
[ "$(id -u)" = 0 ] || die "запускать от root"
grep -q 'VERSION_ID="24.04"' /etc/os-release || die "нужна Ubuntu 24.04"
for f in avtodom.service Caddyfile.template env.example backup.sh backup-serve.sh restore.sh deploy.sh verify.sql; do
  [ -f "$HERE/$f" ] || die "нет $HERE/$f — запускайте из каталога deploy/cloud"
done

CONF=/etc/avtodom/deploy.conf
ENVF=/etc/avtodom/env
install -d -m 755 /etc/avtodom
# Домен, почта и источник кода — из прошлого запуска, если не заданы.
if [ -f "$CONF" ]; then
  # shellcheck source=/dev/null
  . "$CONF"
  domain=${domain:-${DOMAIN:-}}
  email=${email:-${EMAIL:-}}
  repo=${repo:-${REPO_URL:-}}
fi
repo=${repo:-/srv/avtodom.git}
[ -n "$domain" ] || die "укажите --domain (например avtodom.kg)"
[[ "$domain" =~ ^[a-z0-9]([a-z0-9.-]*[a-z0-9])?$ && "$domain" == *.* ]] || die "домен — латиница, цифры, точки и дефисы: $domain"
[ -z "$email" ] || [[ "$email" =~ ^[^[:space:]@]+@[^[:space:]@]+$ ]] || die "неверная почта: $email"

export DEBIAN_FRONTEND=noninteractive
as_postgres() { runuser -u postgres -- "$@"; }
BUILD_HOME=/var/lib/avtodom-build
as_build() {
  runuser -u avtodom-build -- env HOME="$BUILD_HOME" \
    PATH="$BUILD_HOME/.cargo/bin:/opt/node/bin:/usr/local/bin:/usr/bin:/bin" "$@"
}

say "Пакеты"
apt-get update -q
# Порядок строк в базе (сортировка по имени) — en_US.UTF-8, как у PostgreSQL в CI: база avtodom
# создаётся с ней (--locale), поэтому локаль генерируется до установки PostgreSQL. Сам скрипт
# работает в C.UTF-8: уже запущенный процесс новую локаль не увидит.
apt-get install -y -q --no-install-recommends locales
if ! locale -a | grep -ix 'en_US\.utf8' > /dev/null; then
  locale-gen en_US.UTF-8 > /dev/null
fi
locale -a | grep -ix 'en_US\.utf8' > /dev/null || die "локаль en_US.UTF-8 не создалась"
# Прошлый запуск мог оборвать настройку пакета (например, кластер не создался из-за локали).
dpkg --configure -a
apt-get install -y -q --no-install-recommends \
  ca-certificates curl gnupg git openssh-server ufw unattended-upgrades \
  postgresql-16 postgresql-client-16 tzdata xz-utils openssl build-essential pkg-config
# Кластер создаёт сам пакет (с локалью системы, /etc/default/locale). Не создал — создаём.
if ! pg_lsclusters -h | awk '$1 == 16 && $2 == "main"' | grep -q .; then
  echo "Кластер PostgreSQL 16/main не создан при установке пакета — создаю."
  pg_createcluster 16 main --locale=en_US.UTF-8 --start > /dev/null
fi

say "Часовой пояс Asia/Bishkek"
if [ "$systemd" = 1 ]; then
  timedatectl set-timezone Asia/Bishkek
else
  ln -sf /usr/share/zoneinfo/Asia/Bishkek /etc/localtime
  echo Asia/Bishkek > /etc/timezone
fi

say "Пользователи"
id avtodom > /dev/null 2>&1 || useradd --system --home-dir /var/lib/avtodom --create-home --shell /usr/sbin/nologin avtodom
id avtodom-build > /dev/null 2>&1 || useradd --system --home-dir /var/lib/avtodom-build --create-home --shell /bin/bash avtodom-build
id avtodom-pull > /dev/null 2>&1 || useradd --system --home-dir /var/lib/avtodom-pull --create-home --shell /bin/sh --groups avtodom avtodom-pull
chmod 750 /var/lib/avtodom

say "Каталоги"
install -d -m 755 -o root -g root /opt/avtodom /opt/avtodom/releases /opt/avtodom/scripts
install -d -m 750 -o root -g avtodom /var/backups/avtodom
# Программа и клиент — ссылки на текущую версию: deploy.sh меняет только ссылку current.
for d in bin web; do
  if [ -e "/opt/avtodom/$d" ] && [ ! -L "/opt/avtodom/$d" ]; then
    die "/opt/avtodom/$d — не ссылка; перенесите его и запустите снова"
  fi
  ln -sfn "current/$d" "/opt/avtodom/$d"
done
for f in backup.sh backup-serve.sh restore.sh deploy.sh; do install -m 755 "$HERE/$f" "/opt/avtodom/scripts/$f"; done
install -m 644 "$HERE/verify.sql" /opt/avtodom/scripts/verify.sql

say "Источник кода: $repo"
if [ "$repo" = /srv/avtodom.git ] && [ ! -d /srv/avtodom.git ]; then
  git init -q --bare --initial-branch=main /srv/avtodom.git
fi
# В общий репозиторий пишет root (git push с Мака), читает avtodom-build.
if [ -d /srv/avtodom.git ] && ! git config --system --get-all safe.directory | grep -x /srv/avtodom.git > /dev/null; then
  git config --system --add safe.directory /srv/avtodom.git
fi
{
  echo "# Пишет setup.sh. Домен, почта для Let's Encrypt и откуда deploy.sh берёт код."
  printf 'DOMAIN=%q\nEMAIL=%q\nREPO_URL=%q\n' "$domain" "$email" "$repo"
} | put_file "$CONF" 644 || true

say "PostgreSQL 16"
pg_changed=0
if put_file /etc/postgresql/16/main/conf.d/avtodom.conf 644 <<'PGCONF'; then pg_changed=1; fi
# Avtodom: сервер 4 ГБ, одна база. Слушает только localhost (по умолчанию).
shared_buffers = 512MB
effective_cache_size = 2GB
maintenance_work_mem = 128MB
work_mem = 16MB
PGCONF
if [ "$systemd" = 1 ]; then
  systemctl enable --now postgresql
  [ "$pg_changed" = 0 ] || systemctl restart postgresql
else
  if pg_lsclusters -h | awk '$1 == 16 && $2 == "main" && $4 == "online"' | grep -q .; then
    [ "$pg_changed" = 0 ] || pg_ctlcluster 16 main restart
  else
    pg_ctlcluster 16 main start
  fi
fi
for _ in $(seq 30); do as_postgres pg_isready -q && break; sleep 1; done
as_postgres pg_isready -q || die "PostgreSQL не запустился"

# Пароль базы: прежний из /etc/avtodom/env, иначе новый. В командную строку (ps) он не попадает:
# psql получает его через stdin, awk — через окружение.
pw=''
if [ -f "$ENVF" ]; then
  pw=$(sed -nE 's#^DATABASE_URL=postgres://avtodom:([^@]+)@.*#\1#p' "$ENVF" | head -n 1)
fi
[ -n "$pw" ] || pw=$(openssl rand -hex 24)
[[ "$pw" =~ ^[A-Za-z0-9._~-]+$ ]] || die "пароль в DATABASE_URL ($ENVF) — только латиница и цифры; исправьте строку или удалите её"
{
  printf '\\set pw %s\n' "$pw"
  cat <<'SQL'
select format('create role avtodom login password %L', :'pw')
where not exists (select 1 from pg_roles where rolname = 'avtodom')
\gexec
select format('alter role avtodom login nosuperuser nocreatedb nocreaterole password %L', :'pw')
\gexec
SQL
} | as_postgres psql -X -q -v ON_ERROR_STOP=1 -d postgres
if [ "$(as_postgres psql -X -At -d postgres -c "select 1 from pg_database where datname = 'avtodom'")" != 1 ]; then
  as_postgres createdb -O avtodom --template=template0 --locale=en_US.UTF-8 avtodom
fi

say "Настройки сервера: $ENVF"
env_changed=0
# Строка ИМЯ=значение в $ENVF: заменить (повторы убрать) или дописать. Код 0 — файл изменён.
env_set() {
  local tmp rc=0
  tmp=$(mktemp /etc/avtodom/.env.XXXXXX)
  K=$1 V=$2 awk '
    BEGIN { k = ENVIRON["K"] "="; line = k ENVIRON["V"] }
    index($0, k) == 1 && !done { if ($0 != line) changed = 1; print line; done = 1; next }
    index($0, k) == 1 { changed = 1; next }
    { print }
    END { if (!done) { print line; changed = 1 }; exit (changed ? 10 : 0) }
  ' "$ENVF" > "$tmp" || rc=$?
  if [ "$rc" = 10 ]; then cat "$tmp" > "$ENVF"; fi
  rm -f "$tmp"
  [ "$rc" = 10 ] || { [ "$rc" = 0 ] || die "не удалось записать $1 в $ENVF"; return 1; }
}
if [ ! -f "$ENVF" ]; then
  (umask 077 && cp "$HERE/env.example" "$ENVF")
  echo "Создан $ENVF — ключи (бот, банк, WhatsApp) вписать туда, см. README."
  env_changed=1
fi
chown root:root "$ENVF"
chmod 600 "$ENVF"
if env_set DATABASE_URL "postgres://avtodom:$pw@127.0.0.1:5432/avtodom"; then env_changed=1; fi
if env_set PUBLIC_URL "https://$domain"; then env_changed=1; echo "PUBLIC_URL=https://$domain"; fi
# BIND_ADDR и WEB_DIR задаёт служба; строки EnvironmentFile перекрыли бы их.
for k in BIND_ADDR WEB_DIR; do
  if grep -qE "^[[:space:]]*(export[[:space:]]+)?$k=" "$ENVF"; then
    echo "ВНИМАНИЕ: в $ENVF задан $k — его задаёт avtodom.service; строка закомментирована."
    sed -i -E "s/^([[:space:]]*(export[[:space:]]+)?$k=)/# задаёт avtodom.service: \1/" "$ENVF"
    env_changed=1
  fi
done

say "Caddy (HTTPS)"
if [ ! -s /usr/share/keyrings/caddy-stable-archive-keyring.gpg ]; then
  curl -1sLf https://dl.cloudsmith.io/public/caddy/stable/gpg.key \
    | gpg --batch --yes --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
  chmod 644 /usr/share/keyrings/caddy-stable-archive-keyring.gpg
fi
if [ ! -s /etc/apt/sources.list.d/caddy-stable.list ]; then
  curl -1sLf https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt > /etc/apt/sources.list.d/caddy-stable.list
  chmod 644 /etc/apt/sources.list.d/caddy-stable.list
  apt-get update -q
fi
apt-get install -y -q caddy
caddyfile=$(sed "s/__DOMAIN__/$domain/" "$HERE/Caddyfile.template")
if [ -n "$email" ]; then
  caddyfile=${caddyfile//__EMAIL__/$email}
else
  # Без почты — без глобального блока: сертификат выдаётся и так.
  caddyfile=$(sed '/^{$/,/^}$/d' <<< "$caddyfile")
fi
install -d -m 755 -o caddy -g caddy /var/log/caddy
printf '%s\n' "$caddyfile" > /etc/caddy/Caddyfile.check
caddy validate --adapter caddyfile --config /etc/caddy/Caddyfile.check > /dev/null 2>&1 \
  || { caddy validate --adapter caddyfile --config /etc/caddy/Caddyfile.check || true; rm -f /etc/caddy/Caddyfile.check; die "Caddyfile не прошёл проверку"; }
rm -f /etc/caddy/Caddyfile.check
caddy_changed=0
if printf '%s\n' "$caddyfile" | put_file /etc/caddy/Caddyfile 644; then caddy_changed=1; fi
if [ "$systemd" = 1 ]; then
  systemctl enable caddy
  if [ "$caddy_changed" = 1 ] || ! systemctl is-active -q caddy; then systemctl reload-or-restart caddy; fi
fi

say "Служба avtodom и ночная копия"
unit_changed=0
if put_file /etc/systemd/system/avtodom.service 644 < "$HERE/avtodom.service"; then unit_changed=1; fi
put_file /etc/systemd/system/avtodom-backup.service 644 <<'UNIT' || true
[Unit]
Description=Avtodom: копия базы
After=postgresql.service
Requires=postgresql.service

[Service]
Type=oneshot
ExecStart=/opt/avtodom/scripts/backup.sh
UNIT
put_file /etc/systemd/system/avtodom-backup.timer 644 <<'UNIT' || true
[Unit]
Description=Avtodom: копия базы каждую ночь в 02:30 (Бишкек)

[Timer]
OnCalendar=*-*-* 02:30:00 Asia/Bishkek
RandomizedDelaySec=5min
Persistent=true

[Install]
WantedBy=timers.target
UNIT
install -d -m 755 /etc/systemd/journald.conf.d
journal_changed=0
if printf '[Journal]\nSystemMaxUse=500M\n' | put_file /etc/systemd/journald.conf.d/avtodom.conf 644; then journal_changed=1; fi
if [ "$systemd" = 1 ]; then
  systemctl daemon-reload
  [ "$journal_changed" = 0 ] || systemctl restart systemd-journald
  systemctl enable --now avtodom-backup.timer
  # Автозапуск сервера включается один раз; запускает его первая выкатка (deploy.sh). Выключенный
  # потом (systemctl disable avtodom — возврат на Мак) setup.sh снова не включает.
  if [ ! -f /etc/avtodom/.autostart-set ]; then
    systemctl enable avtodom
    touch /etc/avtodom/.autostart-set
  fi
  # Перезапуск — только если изменились служба или настройки и сервер сейчас работает.
  if [ "$unit_changed" = 1 ] || [ "$env_changed" = 1 ]; then
    if systemctl is-active -q avtodom; then
      echo "Служба или настройки изменились — перезапускаю avtodom."
      systemctl try-restart avtodom
    fi
  fi
else
  echo "(--no-systemd: службы не включаются; служба изменена: $unit_changed, настройки: $env_changed)"
fi

say "Автообновления безопасности"
put_file /etc/apt/apt.conf.d/20auto-upgrades 644 <<'APT' || true
APT::Periodic::Update-Package-Lists "1";
APT::Periodic::Unattended-Upgrade "1";
APT::Periodic::AutocleanInterval "7";
APT
put_file /etc/apt/apt.conf.d/52avtodom-upgrades 644 <<'APT' || true
// Avtodom: перезагрузка после обновления ядра — ночью, касса в это время работает без сети.
Unattended-Upgrade::Automatic-Reboot "true";
Unattended-Upgrade::Automatic-Reboot-Time "04:30";
APT
[ "$systemd" = 0 ] || systemctl enable --now unattended-upgrades

say "SSH: вход только по ключу"
if grep -qE '^(ssh-|ecdsa-|sk-)' /root/.ssh/authorized_keys 2> /dev/null; then
  # Имя 00-… — раньше 50-cloud-init.conf: sshd берёт первое встреченное значение.
  if put_file /etc/ssh/sshd_config.d/00-avtodom.conf 644 <<'SSHD'; then
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitRootLogin prohibit-password
SSHD
    install -d -m 755 /run/sshd
    if ! sshd -t; then
      rm -f /etc/ssh/sshd_config.d/00-avtodom.conf
      die "проверка sshd не прошла — настройка SSH не изменена"
    fi
    [ "$systemd" = 0 ] || systemctl try-reload-or-restart ssh
  fi
else
  echo "ВНИМАНИЕ: у root нет SSH-ключа (/root/.ssh/authorized_keys) — вход по паролю НЕ выключен."
  echo "Добавьте ключ (README, раздел 1.2, «SSH-ключ») и запустите setup.sh ещё раз."
fi

say "Ключ Мака для копий (avtodom-pull)"
install -d -m 700 -o avtodom-pull -g avtodom-pull /var/lib/avtodom-pull/.ssh
if [ -n "$pull_key" ]; then
  key=$(grep -E '^(ssh-|ecdsa-|sk-)' "$pull_key" | head -n 1)
  [ -n "$key" ] || die "в $pull_key нет открытого SSH-ключа"
  echo "command=\"/opt/avtodom/scripts/backup-serve.sh\",restrict $key" > /var/lib/avtodom-pull/.ssh/authorized_keys
  chown avtodom-pull:avtodom-pull /var/lib/avtodom-pull/.ssh/authorized_keys
  chmod 600 /var/lib/avtodom-pull/.ssh/authorized_keys
  echo "Мак может забирать копии: ssh avtodom-pull@$domain (только чтение /var/backups/avtodom)."
else
  echo "Ключ Мака не задан (--pull-key-file) — копии на Мак пока не забираются."
fi

# Порт SSH — тот, что слушает sshd, и порт текущего входа: иначе ufw закрыл бы вход.
install -d -m 755 /run/sshd
ssh_ports=$( { sshd -T 2> /dev/null | awk '$1 == "port" { print $2 }'; [ -z "${SSH_CONNECTION:-}" ] || echo "${SSH_CONNECTION##* }"; } \
  | grep -E '^[0-9]+$' | sort -un | tr '\n' ' ')
if [ -z "$ssh_ports" ]; then
  echo "ВНИМАНИЕ: порт sshd не определился (sshd -T) — открываю 22."
  ssh_ports='22 '
fi
say "Брандмауэр: SSH ${ssh_ports% }, 80, 443"
if [ "$systemd" = 1 ]; then
  ufw default deny incoming > /dev/null
  ufw default allow outgoing > /dev/null
  for p in $ssh_ports; do ufw allow "$p/tcp" > /dev/null; done
  ufw allow 80/tcp > /dev/null
  ufw allow 443 > /dev/null
  ufw --force enable
else
  echo "(--no-systemd: ufw не включается)"
fi

say "Подкачка 2 ГБ (сборка сервера на 4 ГБ памяти)"
if [ "$systemd" = 0 ]; then
  echo "(--no-systemd: пропущено)"
elif [ -n "$(swapon --show --noheadings)" ]; then
  echo "уже есть"
else
  [ -f /swapfile ] || { fallocate -l 2G /swapfile; chmod 600 /swapfile; mkswap /swapfile > /dev/null; }
  swapon /swapfile
  grep -q '^/swapfile ' /etc/fstab || echo '/swapfile none swap sw 0 0' >> /etc/fstab
fi

if [ "$build_tools" = 1 ]; then
  say "Сборка: Rust (rustup) и Node 22 для пользователя avtodom-build"
  if [ ! -x /var/lib/avtodom-build/.cargo/bin/cargo ]; then
    as_build sh -c 'cd && curl --proto =https --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y -q --profile minimal --default-toolchain stable'
  fi
  if ! /opt/node/bin/node --version 2> /dev/null | grep -q '^v22\.'; then
    case "$(uname -m)" in
      x86_64) arch=x64 ;;
      aarch64) arch=arm64 ;;
      *) die "неизвестная архитектура $(uname -m)" ;;
    esac
    tmp=$(mktemp -d)
    base=https://nodejs.org/dist/latest-v22.x
    curl -fsSL "$base/SHASUMS256.txt" -o "$tmp/SHASUMS256.txt"
    file=$(awk -v a="linux-$arch.tar.xz" '$2 ~ a"$" {print $2}' "$tmp/SHASUMS256.txt" | head -n 1)
    [ -n "$file" ] || die "не найден Node 22 для linux-$arch"
    curl -fsSL "$base/$file" -o "$tmp/$file"
    (cd "$tmp" && grep " $file\$" SHASUMS256.txt | sha256sum -c --quiet -) || die "контрольная сумма Node не совпала"
    rm -rf /opt/node.new
    mkdir -p /opt/node.new
    tar -xJf "$tmp/$file" -C /opt/node.new --strip-components=1
    rm -rf /opt/node
    mv /opt/node.new /opt/node
    rm -rf "$tmp"
  fi
  echo "cargo: $(as_build cargo --version), node: $(as_build node --version)"
else
  echo "Rust и Node не ставятся (--no-build-tools): выкатка только deploy.sh --bundle."
fi

say "Готово"
cat <<EOF
Домен: https://$domain (запись A домена → адрес этого сервера).
Настройки: $ENVF (root, 600). Копии: /var/backups/avtodom, каждую ночь в 02:30.
Дальше — выкатка версии: /opt/avtodom/scripts/deploy.sh <ревизия>  (README, раздел 2.3).
EOF
