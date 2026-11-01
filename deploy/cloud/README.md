# Avtodom в облаке (VPS)

Программа и база переезжают с Мака точки на арендованный сервер в интернете. Касса, планшет, телефон владельца и администраторов открывают её по постоянному адресу `https://<ваш домен>` из любого места. Бот Телеграма работает круглые сутки, даже когда Мак выключен. Решение — ADR-166.

Что меняется:
- Касса без интернета продаёт как раньше: чеки копятся и уходят, когда связь вернётся.
- Мак точки больше не сервер. Каждый день он забирает к себе копию базы из облака.
- Туннель Cloudflare (`*.trycloudflare.com`) после переезда не нужен.

## Что нужно от владельца

1. Аккаунт Hetzner Cloud с привязанной картой (раздел 1.1). Около €5–6 в месяц.
2. Домен, например `avtodom.kg` или `avtodom-bishkek.com`, и вход в панель, где он куплен (раздел 1.3). От $10 до $30 в год.
3. Запись A домена на адрес сервера (раздел 1.4). Её можно поручить тому, кто выкатывает.
4. Решение по платным копиям Hetzner (Backups, +20% к цене сервера). Мы советуем включить.
5. Вечер переезда: смена закрыта, около часа никто не пробивает чеки.
6. Ключи остаются прежними (бот, банк Optima, WhatsApp), новые получать не нужно. Если банк попросит адрес сервера, дать ему IPv4 из раздела 1.2.

## 1. Для владельца: что купить и где нажать

### 1.1 Аккаунт Hetzner

1. Открыть **hetzner.com/cloud** и нажать **Sign up** (регистрация).
2. Ввести почту и пароль, подтвердить почту по ссылке из письма.
3. Заполнить имя и адрес и привязать карту Visa/Mastercard или PayPal. Иногда Hetzner просит фото паспорта, проверка длится от нескольких минут до суток.
4. В **Hetzner Console** (console.hetzner.cloud) нажать **+ New project** и назвать проект `Avtodom`.
5. Сервер можно создать самому (раздел 1.2) или пригласить того, кто выкатывает: проект → **Security → Members → Invite member**, его почта, роль **Admin**.

### 1.2 Сервер

1. **SSH-ключ.** Тот, кто выкатывает, присылает строку из файла `~/.ssh/id_ed25519.pub`, она начинается с `ssh-ed25519`. Это открытая часть ключа, её можно пересылать. Проект → **Security → SSH keys → Add SSH key**, вставить строку, имя `admin`.
2. Проект → **Add Server** (или **Create Server**):
   - **Location:** Helsinki или Falkenstein/Nuremberg (Европа). Задержка до Бишкека около 80–120 мс, для кассы этого достаточно.
   - **Image:** Ubuntu 24.04.
   - **Type:** Shared vCPU → x86 (Intel/AMD) → **CX22**: 2 vCPU, 4 ГБ памяти, 40 ГБ диска. Если такого названия нет, взять ближайший тариф с 2 vCPU и 4 ГБ (например, CX23).
   - **Networking:** Public IPv4 и IPv6. IPv4 нужен, он стоит около €0,5 в месяц.
   - **SSH keys:** отметить `admin`. Без ключа на почту придёт пароль root, и вход по паролю останется включённым.
   - **Backups:** по желанию (+20%). Тогда Hetzner каждый день снимает копию всего сервера.
   - **Name:** `avtodom`.
3. Нажать **Create & Buy now**. Через минуту на странице сервера появится **IPv4**, например `65.21.10.20`. Его надо записать.

Оплата помесячная, по часам работы сервера. Удалили сервер — оплата прекратилась.

Можно взять и местный хостинг КР или КЗ: задержка меньше, оплата в сомах или тенге. Подходит любой VPS с **Ubuntu 24.04**, **2 vCPU, 4 ГБ, 40 ГБ**, публичным IPv4 и входом root по SSH-ключу. Дальше всё делается так же.

### 1.3 Домен

| Вариант | Где купить | Цена | Замечания |
|---|---|---|---|
| `.kg` (`avtodom.kg`) | у регистратора зоны .kg (список на domain.kg), в Бишкеке | обычно $20–40 в год, уточнить | нужны паспорт или реквизиты фирмы, оплата в сомах |
| `.com` (`avtodom-bishkek.com`) | Cloudflare Registrar, Namecheap, Porkbun | около $10–12 в год | оплата картой, всё через сайт |
| `.store`, `.shop` и подобные | там же | первый год $1–3 | продление $30–60 в год, не советуем |

Покупается только сам домен. Хостинг, почту и «конструктор сайта», которые предлагают в нагрузку, брать не нужно.

### 1.4 Запись A (домен → сервер)

В панели, где куплен домен: **DNS** (или «Управление DNS», «DNS records») → **Add record**:
- **Type:** `A`
- **Name:** `@` (сам домен) или, например, `kassa` (тогда адрес будет `kassa.avtodom.kg`)
- **Value / IPv4:** адрес сервера из раздела 1.2
- **TTL:** 300 (или Auto)

Запись начинает работать через 5–30 минут. Проверить можно в Терминале Мака: `nslookup avtodom.kg` должен показать адрес сервера. На Cloudflare у записи нужно выключить оранжевое облако (**DNS only**).

### 1.5 Сколько стоит

| Что | В месяц |
|---|---|
| Сервер CX22 (или тот же класс) | около €4–5 |
| IPv4 | около €0,5 |
| Копии Hetzner (по желанию) | около €1 |
| Домен | $1–3 (если пересчитать годовую цену на месяц) |
| **Итого** | **около €6–8** |

## 2. Для того, кто выкатывает

Все команды выполняются из папки репозитория на Маке. `IP` — адрес сервера, `ДОМЕН` — домен владельца.

Мак передаёт серверу по ssh свои настройки языка (`LANG`, `LC_*`), а на свежем сервере таких может не быть: тогда команды на сервере ругаются на язык, а установка PostgreSQL падает. Скрипты из `deploy/cloud` выставляют язык сами. Для остальных команд в каждом новом окне Терминала сначала выполнить:

```bash
export LC_ALL=C.UTF-8 LANG=C.UTF-8
```

### 2.1 Доступ и файлы

```bash
ssh-keygen -t ed25519                      # если ключа ~/.ssh/id_ed25519 ещё нет; .pub — владельцу (1.2)
ssh root@IP 'cat /etc/os-release | head -2' # вход по ключу, без пароля
# Отдельный ключ Мака для копий: им можно только читать копии базы.
ssh-keygen -t ed25519 -f ~/.ssh/avtodom_pull -N '' -C avtodom-pull
scp -r deploy/cloud root@IP:/root/avtodom-cloud
scp ~/.ssh/avtodom_pull.pub root@IP:/root/
```

### 2.2 Подготовка сервера (один раз, 5–15 минут)

```bash
ssh root@IP 'bash /root/avtodom-cloud/setup.sh --domain ДОМЕН --email ПОЧТА --pull-key-file /root/avtodom_pull.pub'
```

`setup.sh` делает следующее:
- ставит пакеты, PostgreSQL 16 и Caddy из официального репозитория;
- создаёт пользователей: `avtodom` (служба), `avtodom-build` (сборка), `avtodom-pull` (копии для Мака);
- создаёт роль и базу `avtodom` с новым паролем и пишет `/etc/avtodom/env` (root, 600);
- пишет Caddyfile с доменом, службу `avtodom.service` и ночную копию (`avtodom-backup.timer`, 02:30);
- включает ufw (порт SSH, 80, 443), автообновления безопасности и вход по SSH только по ключу;
- ставит часовой пояс Бишкека, подкачку 2 ГБ, Rust и Node 22 для сборки.

Повторный запуск ничего не ломает: пароль базы и `/etc/avtodom/env` сохраняются. Изменились файлы в `deploy/cloud`? Скопировать их снова (2.1) и запустить `setup.sh`, домен он помнит сам. Сервер он перезапускает, только если изменились служба или настройки. Новый `--domain` переписывает `PUBLIC_URL`. Выключенную службу (`systemctl disable avtodom`, возврат на Мак, 2.9) `setup.sh` снова не включает.

`BIND_ADDR` и `WEB_DIR` в `/etc/avtodom/env` не вписывать: их задаёт служба. Если они там есть, `setup.sh` закомментирует строки и предупредит.

Без SSH-ключа у root `setup.sh` вход по паролю не выключает и предупреждает об этом.

Запись A (1.4) может появиться и после `setup.sh`: пока её нет, Caddy пробует получить сертификат снова каждые несколько минут. Проверить: `journalctl -u caddy -n 20`.

### 2.3 Код на сервер и первая выкатка

Код идёт с Мака прямо на сервер, в свой репозиторий `/srv/avtodom.git`. GitHub не нужен.

```bash
git remote add cloud root@IP:/srv/avtodom.git   # один раз
git push cloud main
ssh root@IP '/opt/avtodom/scripts/deploy.sh main'
```

Первая сборка на 2 vCPU идёт 15–25 минут, следующие 5–10. Потом надо открыть `https://ДОМЕН`. Должен появиться экран входа с замком без ошибки: сертификат Caddy получил сам. Сейчас база пустая, данные приедут в 2.4.

Вместо своего репозитория можно брать код с GitHub. На сервере `sudo -u avtodom-build ssh-keygen -t ed25519`, открытый ключ добавить в репозиторий (**Settings → Deploy keys**, только чтение), затем `setup.sh --repo git@github.com:anteyko-labs/CTO.git`.

### 2.4 Перенос данных с Мака (вечер переезда)

1. **Касса.** Смена закрыта. В кассе нет чеков в очереди («в очереди 0») и нет отложенных чеков: очередь и отложенные чеки браузер хранит отдельно для каждого адреса, и на новый адрес они не переедут. На планшете приёмки нет неподписанной приёмки.
2. **Остановить сервер на Маке и выключить на нём ключи** (Терминал на Маке, папка репозитория). С этого шага и до конца переезда бот не отвечает: ключ бота работает только на одном сервере (2.5), а облако с ключами ещё не запущено.
   ```bash
   bash deploy/cloud/mac-handover.sh off
   pkill cloudflared                                 # туннель, если был
   ```
   `mac-handover.sh off` останавливает сервер на 8080 и его `caffeinate`, затем закомментирует в `server/.env` строки `TELEGRAM_BOT_TOKEN`, `OPTIMA_API_KEY` и `WHATSAPP_*` (копия файла — `server/.env.bak`). Сам сервер он не запускает. Что сделано, он печатает. То же руками:
   ```bash
   P=$(lsof -t -nP -iTCP:8080 -sTCP:LISTEN); pkill -f "caffeinate .*-w $P\$"; kill -INT $P
   sed -i.bak -E 's/^(export +)?(TELEGRAM_BOT_TOKEN|OPTIMA_API_KEY|WHATSAPP_[A-Z_]*)=/#off-cloud# &/' server/.env
   ```
   Проверка: обе команды ниже ничего не печатают.
   ```bash
   lsof -nP -iTCP:8080 -sTCP:LISTEN
   grep -E '^(export +)?(TELEGRAM_BOT_TOKEN|OPTIMA_API_KEY|WHATSAPP_)' server/.env
   ```
3. **Копия и сверка на Маке:**
   ```bash
   D=$(date +%Y%m%d)
   pg_dump -h localhost -U postgres -Fc -d avtodom -f ~/avtodom-backups/avtodom-$D-to-cloud.dump
   psql -X -At -v ON_ERROR_STOP=1 -h localhost -U postgres -d avtodom -f deploy/cloud/verify.sql > ~/avtodom-backups/verify-mac-$D.txt
   scp ~/avtodom-backups/avtodom-$D-to-cloud.dump root@IP:/root/
   ```
4. **Восстановить на сервере, пока без запуска:**
   ```bash
   ssh root@IP "/opt/avtodom/scripts/restore.sh /root/avtodom-$D-to-cloud.dump --yes --no-start"
   ssh root@IP 'runuser -u postgres -- psql -X -At -v ON_ERROR_STOP=1 -d avtodom -f /opt/avtodom/scripts/verify.sql' > ~/avtodom-backups/verify-cloud-$D.txt
   diff ~/avtodom-backups/verify-mac-$D.txt ~/avtodom-backups/verify-cloud-$D.txt && echo 'ЧИСЛА СОВПАЛИ'
   ```
   Должно появиться «ЧИСЛА СОВПАЛИ». `verify.sql` сравнивает:
   - остаток товара по филиалам (количество и себестоимость);
   - каждую кассу и счёт (остаток и сумма движений);
   - долги клиентов и поставщикам;
   - чеки (число, сумма, последний номер и время);
   - журнал и число строк каждой таблицы.

   Если есть хоть одна разница, дальше не идти: вернуть Мак (шаг 8).
5. **Ключи.** Перенести значения из `server/.env` Мака (строки `#off-cloud# …` и остальные ключи) в `/etc/avtodom/env` на сервере:
   ```bash
   ssh -t root@IP nano /etc/avtodom/env
   ```
   - бот: `TELEGRAM_BOT_TOKEN`;
   - сервис разбора: `ASSISTANT_API_KEY`, `ASSISTANT_MODEL`, `ASSISTANT_INVOICE_MODEL`, `ASSISTANT_API_URL`, `ASSISTANT_DAILY_LIMIT`;
   - банк: `OPTIMA_API_KEY`, `OPTIMA_API_URL`;
   - WhatsApp: `WHATSAPP_TOKEN`, `WHATSAPP_PHONE_ID`, `WHATSAPP_API_URL`.

   Значения вписываются без `#off-cloud#` и без кавычек. Не переносить: `DATABASE_URL`, `BIND_ADDR`, `WEB_DIR`, `TRUST_CF_HEADER`, пароль первого входа. `PUBLIC_URL` уже равен `https://ДОМЕН`.
6. **Запуск в облаке.** Сначала ещё раз убедиться, что на Маке сервер остановлен и ключи выключены (проверка из шага 2). Затем запустить и дождаться ответа. Новая версия при запуске сама применит свои миграции, это может занять до 5 минут:
   ```bash
   ssh root@IP 'systemctl start avtodom && /opt/avtodom/scripts/deploy.sh --status --wait'
   ```
   Дальше идти, только если появилось «Сервер отвечает: да». Если «НЕТ» (команда печатает журнал сервера), вернуть Мак (шаг 8).
7. **Вторая сверка и проверка.**
   ```bash
   ssh root@IP 'runuser -u postgres -- psql -X -At -d avtodom -f /opt/avtodom/scripts/verify.sql' > ~/avtodom-backups/verify-cloud2-$D.txt
   diff <(grep -vE '^(строк|миграции|журнал) ' ~/avtodom-backups/verify-mac-$D.txt) \
        <(grep -vE '^(строк|миграции|журнал) ' ~/avtodom-backups/verify-cloud2-$D.txt) && echo 'СУММЫ СОВПАЛИ'
   ```
   Должно появиться «СУММЫ СОВПАЛИ». Если нет, вернуть Мак (шаг 8). Затем владелец входит на `https://ДОМЕН` прежним логином и паролем. «Остатки», «Деньги» и «Долги» показывают то же, что вчера на Маке. Бот отвечает на кнопку: привязки к боту лежат в базе и переехали вместе с ней.
8. **Если суммы не совпали или сервер в облаке не поднялся — вернуть Мак.** База Мака в этот вечер не менялась: с неё только сняли копию. Касса ещё смотрит на Мак (её переключают в 2.6).
   ```bash
   ssh root@IP 'systemctl stop avtodom && systemctl disable avtodom'
   bash deploy/cloud/mac-handover.sh on --host root@IP
   ```
   `mac-handover.sh on` проверяет по ssh, что служба в облаке остановлена и выключена, и только тогда раскомментирует ключи в `server/.env`. Затем запустить сервер Мака командой из 2.9 и туннель, если он был. Причину искать в журнале: `ssh root@IP 'journalctl -u avtodom -n 100 --no-pager'`.

### 2.5 Бот Телеграма: только один сервер

Ключ бота работает только в одном месте. Если сервер запущен с ним и на Маке, и в облаке, они перехватывают друг у друга сообщения. Сообщения клиентам и опрос банка тогда тоже идут дважды. Поэтому:
- до запуска в облаке ключи на Маке выключаются: `mac-handover.sh off` (2.4, шаг 2);
- после переезда сервер на Маке **не запускается** с `TELEGRAM_BOT_TOKEN`, `OPTIMA_API_KEY` и ключами WhatsApp. Пробная копия (как 8090) — только без них;
- ключ бота с этого момента есть только в `/etc/avtodom/env`. Сменить ключ — `systemctl restart avtodom`;
- ключи на Мак возвращаются только после `systemctl stop avtodom && systemctl disable avtodom` на сервере: `mac-handover.sh on --host root@IP` это проверяет. Пока служба выключена, `deploy.sh` не выкатывает и не откатывает версии, а `setup.sh` её не включает.

### 2.6 Переключить кассу (моноблок Windows)

1. Скопировать на моноблок `deploy/windows/install-cert.ps1` (папка `C:\Avtodom-сертификат` или флешка) и запустить PowerShell от имени администратора:
   ```
   powershell -ExecutionPolicy Bypass -File install-cert.ps1 -Url https://ДОМЕН -NoCert
   ```
   Ярлык «Avtodom касса» на рабочем столе и в автозагрузке теперь открывает новый адрес. Печать без окна остаётся как была. Свой сертификат не нужен, у домена сертификат Let's Encrypt.
2. Закрыть все окна Chrome и открыть кассу ярлыком. Войти администратором и снова включить «Режим кассы на этом устройстве». Пробный чек должен выйти без окна.
3. Старые ярлыки и закладки с `http://…:8080` удалить: по старому адресу касса продавала бы в очередь, которую новый адрес не видит.
4. На планшете приёмки и телефонах открыть `https://ДОМЕН` и нажать «Добавить на главный экран». На планшете снова включить «Режим приёмки на этом устройстве».
5. Проверка без сети: выдернуть интернет у роутера, касса продаёт. Вернуть интернет — чеки уходят, счётчик очереди становится 0.

### 2.7 Копии базы

- Сервер: каждую ночь в 02:30 копия попадает в `/var/backups/avtodom/avtodom-ГГГГ-ММ-ДД.dump`. Хранятся 14 последних. Копии перед выкаткой и восстановлением (`…-before-…`) хранятся 30 дней. Не удалась копия — владельцу приходит «Копия базы не создана» (уведомления и бот). Журнал: `journalctl -u avtodom-backup`. Ручная копия: `/opt/avtodom/scripts/backup.sh`.
- **Мак забирает копии** (вторая копия вне облака):
  ```bash
  DOMAIN=avtodom.kg                                  # свой домен
  mkdir -p ~/avtodom-cloud ~/avtodom-backups/cloud
  cp deploy/cloud/pull-backup-to-mac.sh ~/avtodom-cloud/
  AVTODOM_CLOUD_HOST=$DOMAIN bash ~/avtodom-cloud/pull-backup-to-mac.sh   # первый раз руками
  sed -e "s/ПОЛЬЗОВАТЕЛЬ/$(whoami)/g" -e "s/ДОМЕН/$DOMAIN/" deploy/cloud/kg.avtodom.pull-backup.plist \
    > ~/Library/LaunchAgents/kg.avtodom.pull-backup.plist
  launchctl load ~/Library/LaunchAgents/kg.avtodom.pull-backup.plist
  ```
  Скрипт запускается каждый день в 04:00, а если Мак спал — при пробуждении. Каждую копию он сверяет по размеру и sha256 и хранит 30 последних в `~/avtodom-backups/cloud`. Если свежей копии нет, в `pull.log` появляется строка «ВНИМАНИЕ».
- **Проверка раз в месяц** на Маке (рабочую базу не трогает):
  ```bash
  F=$(ls -1 ~/avtodom-backups/cloud/avtodom-*.dump | tail -n 1)
  createdb -h localhost -U postgres avtodom_check && pg_restore -h localhost -U postgres -d avtodom_check --no-owner "$F" \
    && psql -X -At -h localhost -U postgres -d avtodom_check -c "select count(*), max(created_at) from sales"; dropdb -h localhost -U postgres avtodom_check
  ```
  Последний чек должен быть вечером дня копии.
- **Восстановление на сервере:** `/opt/avtodom/scripts/restore.sh /var/backups/avtodom/avtodom-ГГГГ-ММ-ДД.dump`. Скрипт попросит ввести `ВОССТАНОВИТЬ`. Перед восстановлением он сохраняет копию текущей базы, а после него запускает сверку и сервер.

### 2.8 Обновление и откат версии

```bash
git push cloud main && ssh root@IP '/opt/avtodom/scripts/deploy.sh main'
ssh root@IP '/opt/avtodom/scripts/deploy.sh --status'     # что работает, версии, копии
ssh root@IP '/opt/avtodom/scripts/deploy.sh --rollback'   # вернуть прежнюю (спросит ОТКАТ)
```

Как идёт выкатка (в порядке `deploy.sh`):
1. Сборка на сервере или распаковка архива (`--bundle`, в нём обязателен `migrations.txt`) в `/opt/avtodom/releases/<дата>-<коммит>`. Проверка: программа для Linux, есть `web/index.html`. Не собралось — ничего не меняется, работает прежняя версия.
2. Остановка сервера. Касса с этой минуты копит чеки в очереди.
3. Копия базы перед выкаткой (`…-before-deploy-…`). Её снимают после остановки, поэтому ни один чек в обход копии не попадёт. Не удалась — прежняя версия запускается снова, выкатка отменяется.
4. Переключение ссылки на новую версию и запуск. Сервер сам применяет миграции.
5. Проверка: страница входа и API должны ответить в течение 5 минут.
6. Не ответила — сервер останавливается. Если миграции успели изменить базу, она возвращается из копии шага 3. Затем запускается прежняя версия.

Пока служба выключена (`systemctl disable avtodom`, работает Мак), `deploy.sh` ничего не выкатывает и не откатывает.

`--rollback` возвращает базу, только если прежняя версия не знает её миграций. В этом случае всё, что внесено после выкатки, пропадёт (копия текущей базы останется). Вернуть новую версию: `deploy.sh --activate <версия из --status>`.

Касса подхватывает новую версию сама, как на точке.

**Сборка без сервера** (если на VPS мало памяти или нужна та же программа, что прошла CI): собрать Linux-программу в Docker на Маке. На Mac с Apple-процессором это идёт долго, сборка под x86 эмулируется.

```bash
docker build --platform linux/amd64 -t avtodom:$(git rev-parse --short HEAD) .
id=$(docker create avtodom:$(git rev-parse --short HEAD)); rm -rf /tmp/b && mkdir -p /tmp/b/bin
docker cp $id:/app/avtodom-server /tmp/b/bin/ && docker cp $id:/app/web /tmp/b/web && docker rm $id
git rev-parse HEAD > /tmp/b/commit.txt
ls server/migrations/*.sql | sed -E 's#.*/0*([0-9]+)_.*#\1#' | tr '\n' ' ' > /tmp/b/migrations.txt
tar -czf /tmp/avtodom-bundle.tar.gz -C /tmp/b . && scp /tmp/avtodom-bundle.tar.gz root@IP:/root/
ssh root@IP '/opt/avtodom/scripts/deploy.sh --bundle /root/avtodom-bundle.tar.gz'
```

### 2.9 Откат переезда (вернуть Мак)

1. На сервере остановить программу и убрать её из автозапуска, чтобы бот там больше не работал:
   ```bash
   ssh root@IP 'systemctl stop avtodom && systemctl disable avtodom'
   ```
2. Вернуть ключи на Мак (проверяет, что служба в облаке остановлена и выключена):
   ```bash
   bash deploy/cloud/mac-handover.sh on --host root@IP
   ```
   Без связи с сервером: выключить его в панели Hetzner (**Power off**), затем `mac-handover.sh on --yes`. То же руками: `sed -i.bak -E 's/^#off-cloud# //' server/.env`.
3. **В облаке ещё ничего не пробивали** (откат в тот же вечер). База Мака не менялась с момента копии, сразу шаг 5.
4. **В облаке уже работали.** Сначала перенести базу обратно:
   ```bash
   ssh root@IP '/opt/avtodom/scripts/backup.sh before-back-to-mac' | tail -n 1   # путь копии
   scp root@IP:/var/backups/avtodom/<файл из строки выше> ~/avtodom-backups/
   pg_dump -h localhost -U postgres -Fc -d avtodom -f ~/avtodom-backups/avtodom-$(date +%Y%m%d)-before-back.dump
   dropdb -h localhost -U postgres --force avtodom && createdb -h localhost -U postgres avtodom
   pg_restore -h localhost -U postgres -d avtodom --no-owner --no-acl ~/avtodom-backups/<файл>
   ```
   Запускать на Маке тогда **ту же версию, что работала в облаке** (`deploy.sh --status`, коммит). Прежняя сборка Мака не запустится на базе с более новыми миграциями.
5. **Запустить сервер Мака на 8080** (папка репозитория). `<программа>` — сохранённая копия `avtodom-server` нужной сборки, `<клиент>` — копия `web/dist` той же сборки. Ключи берутся из `server/.env` Мака, в командной строке их нет:
   ```bash
   cd server
   set -a; . ./.env; set +a
   TRUST_CF_HEADER=1 BIND_ADDR=0.0.0.0:8080 WEB_DIR=<клиент> nohup <программа> >> ~/avtodom-backups/srv8080.log 2>&1 &
   caffeinate -s -i -w $! &
   cd ..
   ```
   `TRUST_CF_HEADER=1` нужен, только если снова работает туннель Cloudflare. `caffeinate` не даёт Маку уснуть, пока жив сервер. Проверка: `curl -s -o /dev/null -w '%{http_code}\n' http://localhost:8080/` печатает 200.
6. Касса: `install-cert.ps1 -Url http://<адрес Мака>:8080 -NoCert`, планшет и телефоны — тоже на старый адрес.

### 2.10 Обслуживание

- Журнал сервера: `journalctl -u avtodom -f`, HTTPS: `/var/log/caddy/avtodom.log`, копии: `journalctl -u avtodom-backup`.
- Настройки: `/etc/avtodom/env`, после правки `systemctl restart avtodom`.
- Обновления безопасности ставятся сами. Если нужна перезагрузка, сервер перезагружается в 04:30, касса эту минуту работает без сети. Проверить: `cat /var/run/reboot-required 2>/dev/null`.
- Место на диске: `df -h /`. Копии больше 10 ГБ — владельцу приходит «Копии базы занимают много места».
- Доступ администраторов — только через программу, по логину и паролю с их правами. SSH на сервер есть только у того, кто выкатывает.
