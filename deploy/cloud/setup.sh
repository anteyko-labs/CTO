#!/usr/bin/env bash
# Подготовка облачного сервера Avtodom (Ubuntu 24.04, запуск от root). Повторный запуск безопасен:
# пароль базы, /etc/avtodom/env и выкаченные версии сохраняются, файлы настроек переписываются
# из этого каталога. Решение — ADR-{cloud.1}, инструкция — deploy/cloud/README.md.
#   bash setup.sh --domain avtodom.kg [--email почта@для.letsencrypt] [--pull-key-file mac.pub]
#                 [--repo URL] [--no-build-tools] [--no-systemd]
# --pull-key-file   открытый ключ Мака точки: ему — только чтение копий базы (backup-serve.sh).
# --repo            откуда deploy.sh берёт код; по умолчанию /srv/avtodom.git (git push с Мака).
# --no-build-tools  не ставить Rust и Node (версии собираются в другом месте: deploy.sh --bundle).
# --no-systemd      проверка в контейнере без systemd: службы не включаются, PostgreSQL
#                   запускается pg_ctlcluster, ufw/swap/sshd не трогаются (конфиги проверяются).
set -euo pipefail

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
apt-get install -y -q --no-install-recommends \
  ca-certificates curl gnupg git openssh-server ufw unattended-upgrades \
  postgresql-16 postgresql-client-16 tzdata locales xz-utils openssl build-essential pkg-config \
  debian-keyring debian-archive-keyring apt-transport-https

# Порядок строк в базе (сортировка по имени) — en_US.UTF-8, как у PostgreSQL в CI.
if ! locale -a | grep -ix 'en_US\.utf8' > /dev/null; then
  locale-gen en_US.UTF-8 > /dev/null
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
} > "$CONF"

say "PostgreSQL 16"
cat > /etc/postgresql/16/main/conf.d/avtodom.conf.new <<'PGCONF'
# Avtodom: сервер 4 ГБ, одна база. Слушает только localhost (по умолчанию).
shared_buffers = 512MB
effective_cache_size = 2GB
maintenance_work_mem = 128MB
work_mem = 16MB
PGCONF
pg_changed=0
if ! cmp -s /etc/postgresql/16/main/conf.d/avtodom.conf.new /etc/postgresql/16/main/conf.d/avtodom.conf; then
  mv /etc/postgresql/16/main/conf.d/avtodom.conf.new /etc/postgresql/16/main/conf.d/avtodom.conf
  pg_changed=1
else
  rm -f /etc/postgresql/16/main/conf.d/avtodom.conf.new
fi
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

# Пароль базы: прежний из /etc/avtodom/env, иначе новый.
pw=''
if [ -f "$ENVF" ]; then
  pw=$(sed -nE 's#^DATABASE_URL=postgres://avtodom:([^@]+)@.*#\1#p' "$ENVF" | head -n 1)
fi
[ -n "$pw" ] || pw=$(openssl rand -hex 24)
as_postgres psql -X -q -v ON_ERROR_STOP=1 -d postgres -v pw="$pw" <<'SQL'
select format('create role avtodom login password %L', :'pw')
where not exists (select 1 from pg_roles where rolname = 'avtodom')
\gexec
select format('alter role avtodom login nosuperuser nocreatedb nocreaterole password %L', :'pw')
\gexec
SQL
if [ "$(as_postgres psql -X -At -d postgres -c "select 1 from pg_database where datname = 'avtodom'")" != 1 ]; then
  as_postgres createdb -O avtodom --template=template0 --locale=en_US.UTF-8 avtodom
fi

say "Настройки сервера: $ENVF"
url="postgres://avtodom:$pw@127.0.0.1:5432/avtodom"
if [ ! -f "$ENVF" ]; then
  umask 077
  sed -e "s#^DATABASE_URL=.*#DATABASE_URL=$url#" -e "s#^PUBLIC_URL=.*#PUBLIC_URL=https://$domain#" \
    "$HERE/env.example" > "$ENVF"
  umask 022
  echo "Создан $ENVF — ключи (бот, банк, WhatsApp) вписать туда, см. README."
elif ! grep -q '^DATABASE_URL=postgres' "$ENVF"; then
  sed -i "s#^DATABASE_URL=.*#DATABASE_URL=$url#" "$ENVF"
  grep -q '^DATABASE_URL=postgres' "$ENVF" || echo "DATABASE_URL=$url" >> "$ENVF"
fi
chown root:root "$ENVF"
chmod 600 "$ENVF"

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
printf '%s\n' "$caddyfile" > /etc/caddy/Caddyfile.new
install -d -m 755 -o caddy -g caddy /var/log/caddy
caddy validate --adapter caddyfile --config /etc/caddy/Caddyfile.new > /dev/null 2>&1 \
  || { caddy validate --adapter caddyfile --config /etc/caddy/Caddyfile.new || true; die "Caddyfile не прошёл проверку"; }
mv /etc/caddy/Caddyfile.new /etc/caddy/Caddyfile
if [ "$systemd" = 1 ]; then
  systemctl enable caddy
  systemctl reload-or-restart caddy
fi

say "Служба avtodom и ночная копия"
install -m 644 "$HERE/avtodom.service" /etc/systemd/system/avtodom.service
cat > /etc/systemd/system/avtodom-backup.service <<'UNIT'
[Unit]
Description=Avtodom: копия базы
After=postgresql.service
Requires=postgresql.service

[Service]
Type=oneshot
ExecStart=/opt/avtodom/scripts/backup.sh
UNIT
cat > /etc/systemd/system/avtodom-backup.timer <<'UNIT'
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
printf '[Journal]\nSystemMaxUse=500M\n' > /etc/systemd/journald.conf.d/avtodom.conf
if [ "$systemd" = 1 ]; then
  systemctl daemon-reload
  systemctl restart systemd-journald
  systemctl enable --now avtodom-backup.timer
  # Сервер включается в автозапуск; запускает его первая выкатка (deploy.sh).
  systemctl enable avtodom
  if [ -x /opt/avtodom/bin/avtodom-server ]; then systemctl restart avtodom; fi
fi

say "Автообновления безопасности"
cat > /etc/apt/apt.conf.d/20auto-upgrades <<'APT'
APT::Periodic::Update-Package-Lists "1";
APT::Periodic::Unattended-Upgrade "1";
APT::Periodic::AutocleanInterval "7";
APT
cat > /etc/apt/apt.conf.d/52avtodom-upgrades <<'APT'
// Avtodom: перезагрузка после обновления ядра — ночью, касса в это время работает без сети.
Unattended-Upgrade::Automatic-Reboot "true";
Unattended-Upgrade::Automatic-Reboot-Time "04:30";
APT
[ "$systemd" = 0 ] || systemctl enable --now unattended-upgrades

say "SSH: вход только по ключу"
if grep -qE '^(ssh-|ecdsa-|sk-)' /root/.ssh/authorized_keys 2> /dev/null; then
  # Имя 00-… — раньше 50-cloud-init.conf: sshd берёт первое встреченное значение.
  cat > /etc/ssh/sshd_config.d/00-avtodom.conf <<'SSHD'
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

say "Брандмауэр: 22, 80, 443"
if [ "$systemd" = 1 ]; then
  ufw default deny incoming > /dev/null
  ufw default allow outgoing > /dev/null
  ufw allow 22/tcp > /dev/null
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
