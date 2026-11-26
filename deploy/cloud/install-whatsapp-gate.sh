#!/bin/bash
# Шлюз WhatsApp по QR-коду (задача 052, ADR-217) на облачном сервере. Запускать от root:
#   bash /root/avtodom-cloud/install-whatsapp-gate.sh [папка-с-кодом]
# Папка — копия репозитория (по умолчанию сборочная /var/lib/avtodom-build/src после deploy.sh).
# Ставит шлюз в /opt/avtodom/wa (npm ci, точные версии из package-lock.json), службу avtodom-wa,
# добавляет в /etc/avtodom/env WA_GATE_URL и случайный WA_GATE_KEY (если их нет) и перезапускает
# avtodom, чтобы сервер увидел шлюз. Повторный запуск обновляет код шлюза; сессия номера остаётся.
set -euo pipefail
SRC=${1:-/var/lib/avtodom-build/src}
GATE=$SRC/deploy/whatsapp-gate
ENVF=/etc/avtodom/env
die() { echo "ОШИБКА: $*" >&2; exit 1; }
[ "$(id -u)" = 0 ] || die "запускать от root"
[ -f "$GATE/gate.mjs" ] || die "нет $GATE/gate.mjs — сначала deploy.sh с ревизией, где есть задача 052"
[ -x /opt/node/bin/npm ] || die "нет Node в /opt/node (setup.sh ставит его для сборки)"
[ -f "$ENVF" ] || die "нет $ENVF"

install -d -o avtodom -g avtodom -m 750 /opt/avtodom/wa /var/lib/avtodom/whatsapp
cp "$GATE/package.json" "$GATE/package-lock.json" "$GATE/gate.mjs" "$GATE/lib.mjs" /opt/avtodom/wa/
chown -R avtodom:avtodom /opt/avtodom/wa
( cd /opt/avtodom/wa && sudo -u avtodom env PATH=/opt/node/bin:/usr/bin:/bin HOME=/opt/avtodom/wa \
    /opt/node/bin/npm ci --omit=dev --no-audit --no-fund ) || die "npm ci не прошёл"

if ! grep -q '^WA_GATE_URL=.' "$ENVF"; then
  cp -p "$ENVF" "$ENVF.bak-before-wa-$(date +%Y%m%d-%H%M%S)"
  sed -i '/^WA_GATE_URL=/d;/^WA_GATE_KEY=/d' "$ENVF"
  {
    echo ""
    echo "# WhatsApp по QR-коду (задача 052): шлюз avtodom-wa на этом сервере."
    echo "WA_GATE_URL=http://127.0.0.1:8091"
    echo "WA_GATE_KEY=$(head -c 32 /dev/urandom | base64 | tr -dc 'A-Za-z0-9' | head -c 40)"
  } >> "$ENVF"
  echo "В $ENVF добавлены WA_GATE_URL и WA_GATE_KEY"
fi

install -m 644 "$SRC/deploy/cloud/avtodom-wa.service" /etc/systemd/system/avtodom-wa.service
systemctl daemon-reload
systemctl enable avtodom-wa >/dev/null
systemctl restart avtodom-wa
systemctl restart avtodom
sleep 3
systemctl is-active avtodom-wa >/dev/null || { journalctl -u avtodom-wa -n 30 --no-pager; die "avtodom-wa не запустился"; }
echo "Готово: шлюз WhatsApp работает. Подключение — «Настройки → Сообщения клиентам → WhatsApp по QR-коду → Подключить»."
