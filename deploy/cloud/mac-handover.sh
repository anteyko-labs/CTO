#!/usr/bin/env bash
# Мак точки при переезде в облако (ADR-166, README 2.4 и 2.9). Ключ бота, банк и WhatsApp
# работают только на ОДНОМ сервере: два сервера с одним ключом перехватывают сообщения бота и шлют
# клиентам и банку всё дважды.
#   mac-handover.sh off [--env server/.env] [--port 8080]
#       перед запуском сервера в облаке: остановить сервер Мака на порту 8080 и его caffeinate,
#       закомментировать в .env TELEGRAM_BOT_TOKEN, OPTIMA_API_KEY и WHATSAPP_* (копия — .env.bak).
#   mac-handover.sh on [--env server/.env] [--host root@IP] [--yes]
#       возврат на Мак — ТОЛЬКО после остановки службы в облаке
#       (ssh root@IP 'systemctl stop avtodom && systemctl disable avtodom'): с --host это
#       проверяется по ssh, без него — спрашивается подтверждение (--yes — подтверждено заранее).
#       Ключи раскомментируются (копия — .env.bak).
# Сервер Мака скрипт не запускает: команда запуска — README 2.9. Значения ключей не печатаются.
set -euo pipefail

HERE=$(cd "$(dirname "$0")" && pwd)
mode=${1:-}
[ $# -eq 0 ] || shift
envf="$HERE/../../server/.env"
port=8080
host=''
yes=0
while [ $# -gt 0 ]; do
  case "$1" in
    --env) envf=${2:?}; shift 2 ;;
    --port) port=${2:?}; shift 2 ;;
    --host) host=${2:?}; shift 2 ;;
    --yes) yes=1; shift ;;
    *) echo "Неизвестный параметр: $1" >&2; exit 2 ;;
  esac
done

die() { echo "ОШИБКА: $*" >&2; exit 1; }
say() { printf '== %s\n' "$*"; }
case "$mode" in
  off | on) ;;
  *) sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
[ -f "$envf" ] || die "нет файла $envf (укажите --env путь/к/server/.env)"
envf=$(cd "$(dirname "$envf")" && pwd)/$(basename "$envf")
[[ "$port" =~ ^[0-9]+$ ]] || die "порт — число: $port"

MARK='#off-cloud# '
# Ключи, которые работают только на одном сервере.
KEYS='TELEGRAM_BOT_TOKEN|OPTIMA_API_KEY|WHATSAPP_[A-Z0-9_]*'
ACTIVE_RE="^[[:space:]]*(export[[:space:]]+)?($KEYS)="
OFF_RE="^$MARK"

# Имена (без значений) строк .env, подходящих под выражение.
names() { { grep -E "$1" "$envf" || true; } | sed -E "s/^($MARK)?[[:space:]]*(export[[:space:]]+)?([A-Z0-9_]+)=.*/\3/" | tr '\n' ' '; }

# Переписать .env через sed-выражение: сначала копия .env.bak, права файла сохраняются.
rewrite_env() {
  local tmp
  cp -p "$envf" "$envf.bak"
  tmp=$(mktemp "$envf.XXXXXX")
  sed -E "$1" "$envf" > "$tmp"
  cat "$tmp" > "$envf"
  rm -f "$tmp"
  echo "Копия прежнего файла: $envf.bak"
}

listening() { lsof -t -nP -iTCP:"$port" -sTCP:LISTEN 2> /dev/null || true; }

if [ "$mode" = off ]; then
  say "1. Сервер Мака на порту $port"
  pids=$(listening)
  if [ -z "$pids" ]; then
    echo "На порту $port никто не слушает — сервер уже остановлен."
  else
    for pid in $pids; do
      echo "Останавливаю: $(ps -o pid=,command= -p "$pid" | cut -c1-120)"
      # caffeinate -w <pid> держит Мак без сна, пока жив сервер.
      cafs=$(pgrep -f "caffeinate .*-w ?$pid\$" || true)
      for c in $cafs; do
        echo "Останавливаю caffeinate: $(ps -o pid=,command= -p "$c")"
        kill "$c" 2> /dev/null || true
      done
      # Сервер завершается штатно по SIGINT (как Ctrl+C): дожидается текущих запросов.
      kill -INT "$pid" 2> /dev/null || true
    done
    for _ in $(seq 30); do [ -n "$(listening)" ] || break; sleep 1; done
    if [ -n "$(listening)" ]; then
      echo "Не остановился за 30 с — SIGTERM."
      for pid in $(listening); do kill "$pid" 2> /dev/null || true; done
      sleep 3
    fi
    [ -z "$(listening)" ] || die "сервер на порту $port не остановился: $(listening)"
    echo "Сервер на порту $port остановлен."
  fi

  say "2. Ключи в $envf"
  active=$(names "$ACTIVE_RE")
  if [ -z "$active" ]; then
    echo "Включённых ключей нет — уже выключены: $(names "$OFF_RE")"
  else
    rewrite_env "s/$ACTIVE_RE/$MARK&/"
    echo "Закомментированы: $active"
  fi
  left=$(names "$ACTIVE_RE")
  [ -z "$left" ] || die "в $envf остались включены: $left"
  say "Готово: сервер Мака остановлен, ключи бота, банка и WhatsApp выключены."
  echo "Сервер Мака теперь можно запускать только для проверок: без ключей он не отвечает в боте и не шлёт сообщений."
  echo "Возврат: mac-handover.sh on — после systemctl stop avtodom && systemctl disable avtodom на сервере в облаке."
  exit 0
fi

# --- on -------------------------------------------------------------------------------------
say "1. Служба в облаке"
if [ -n "$host" ]; then
  state=$(ssh -o BatchMode=yes -o ConnectTimeout=15 "$host" \
    'echo "$(systemctl is-active avtodom 2>/dev/null || true) $(systemctl is-enabled avtodom 2>/dev/null || true)"') \
    || die "нет связи с $host — проверить службу нельзя. Остановите её на сервере или в панели Hetzner (выключить сервер) и запустите с --yes"
  active_state=${state%% *}
  enabled_state=${state##* }
  echo "$host: служба $active_state, автозапуск $enabled_state"
  if [ "$active_state" = active ] || [ "$active_state" = activating ] || [ "$enabled_state" = enabled ]; then
    die "служба в облаке не остановлена. Сначала: ssh $host 'systemctl stop avtodom && systemctl disable avtodom'"
  fi
elif [ "$yes" != 1 ]; then
  [ -t 0 ] || die "нет --host и нет терминала для вопроса: укажите --host root@IP или --yes"
  echo "Служба в облаке должна быть остановлена и выключена:"
  echo "  ssh root@IP 'systemctl stop avtodom && systemctl disable avtodom'"
  read -r -p 'Если это сделано, введите ОСТАНОВЛЕНА: ' answer
  [ "$answer" = 'ОСТАНОВЛЕНА' ] || { echo 'Отменено, ничего не изменено.'; exit 1; }
else
  echo "(--yes: остановка службы в облаке подтверждена)"
fi

say "2. Ключи в $envf"
off=$(names "$OFF_RE")
if [ -z "$off" ]; then
  echo "Выключенных ключей (строки «${MARK}…») нет — возвращать нечего."
else
  rewrite_env "s/$OFF_RE//"
  echo "Раскомментированы: $off"
fi
echo "Включены: $(names "$ACTIVE_RE")"

pids=$(listening)
if [ -n "$pids" ]; then
  echo "ВНИМАНИЕ: на порту $port уже работает сервер (pid $pids) — он запущен без ключей; перезапустите его."
fi
say "Готово: ключи вернулись в $envf. Запустить сервер Мака — команда из README 2.9."
