---
type: arch
status: ready
tier: 3
updated: 2026-10-05
---

# Архитектура ядра

## Структура репозитория

| Путь | Содержимое |
| --- | --- |
| `server/` | Сервер на Rust: Axum, SQLx, миграции в `server/migrations/` |
| `server/src/domain/` | Чистые расчёты без БД: деньги, себестоимость, штрихкоды |
| `server/src/api/` | HTTP-обработчики по модулям |
| `server/src/db/` | Запросы и транзакции |
| `web/` | Клиент: React + Vite + TypeScript + Tailwind |
| `deploy/` | Docker Compose, конфигурация прокси |

## Сервер

- Один бинарник. Раздаёт API под `/api/v1` и собранный клиент из `web/dist`.
- Конфигурация только из переменных окружения: `DATABASE_URL`, `BIND_ADDR`, `WEB_DIR`, `COOKIE_SECURE`, `BOOTSTRAP_OWNER_LOGIN`, `BOOTSTRAP_OWNER_PASSWORD`; бот — `TELEGRAM_BOT_TOKEN` (SPEC-18), вопросы боту текстом — `ASSISTANT_API_KEY`, `ASSISTANT_MODEL`, `ASSISTANT_DAILY_LIMIT`, `ASSISTANT_API_URL` (SPEC-21); накладные по фото — тот же ключ и `ASSISTANT_INVOICE_MODEL` (SPEC-30 §9), работают и без бота; банк Optima Business — `OPTIMA_API_KEY`, `OPTIMA_API_URL` (SPEC-31: QR на сумму чека, опрос статуса каждые 2 с и сверка с выпиской в 06:00 — фоновые задачи сервера; без ключа выключено); сообщения клиентам — `WHATSAPP_TOKEN`, `WHATSAPP_PHONE_ID`, `WHATSAPP_API_URL` (по умолчанию `https://graph.facebook.com/v26.0`) и `PUBLIC_URL` — внешний адрес для ссылки на кабинет (SPEC-32: фоновая задача раз в 30 с в окно 09:00–20:00 — сообщения о закупах фирм, раз в час — напоминания о замене по среднему пробегу; работает, если есть бот или WhatsApp).
- Миграции применяются при старте.
- Ошибки — одно перечисление `AppError` (thiserror), преобразуемое в HTTP-ответ `{ "error": { "code": "...", "message": "..." } }`.
- Lint-ы крейта: `clippy::unwrap_used`, `clippy::expect_used`, `clippy::panic` — `deny` (инвариант 18).

## Соглашения API

- JSON, имена полей `snake_case`.
- Деньги — целые тыйыны (`*_tyiyn`), объёмы — миллилитры (`*_ml`), количество — целые (`qty`). Сервер не принимает и не отдаёт дробных чисел.
- Время — RFC 3339 в UTC.
- Сессия — cookie `avtodom_session` (HttpOnly, SameSite=Strict). В базе хранится только SHA-256 токена.
- Каждый изменяющий запрос несёт заголовок `X-Device-Id` (UUID устройства, создаётся клиентом один раз) и поле `op_id` в теле (ADR-013).
- Коды ошибок: `unauthorized` 401, `forbidden` 403, `not_found` 404, `validation` 422, `conflict` 409, `internal` 500.

## Роли

| Действие | Владелец | Администратор |
| --- | --- | --- |
| Пользователи | да | нет |
| Сотрудники, услуги, категории, товары, поставщики | да | да |
| Цена продажи товара | да | да |
| Цена розлива за литр | считается от цены канистры (ADR-074) | — |
| Приход и его сторно (с закупочными ценами) | да | да |
| Средняя себестоимость, стоимость остатка, себестоимость в чеке | да | нет |
| Продажа и возврат | да | да |
| Проверка остатков (пересчёт из движений) | да | нет |

Поля, недоступные роли, сервер не включает в ответ (инвариант 13).

## Модель данных

Все идентификаторы — UUID v7. Все таблицы учёта содержат `branch_id`.

```
branches (id, name)
users (id, branch_id, login, password_hash, role owner|admin, full_name, active)
sessions (token_hash, user_id, device_id, created_at, expires_at)
operations (op_id, user_id, kind, response jsonb, created_at)          -- идемпотентность
audit_log (id, branch_id, user_id, device_id, action, entity, entity_id, data jsonb, created_at)

employees (id, branch_id, full_name, is_cashier, is_master, active)
services (id, branch_id, name, price_tyiyn, master_fee_tyiyn, active)

categories (id, name, kind oil|filter|battery|other, attributes jsonb)
products (id, category_id, name, brand, article, unit piece|ml, container_ml, attrs jsonb, archived)
product_barcodes (code, product_id, internal)
branch_products (branch_id, product_id, sale_price_tyiyn, pour_price_per_l_tyiyn,
                 min_stock, stock_qty, stock_value_tyiyn, last_cost_qty, last_cost_tyiyn, needs_review)

suppliers (id, name, phone, comment)
receipts (id, branch_id, number, supplier_id, supplier_doc, comment, total_tyiyn,
          reversal_of, user_id, device_id, created_at)
receipt_lines (id, receipt_id, product_id, qty, cost_tyiyn)

sales (id, branch_id, number, kind sale|return, sale_type takeaway|service, cashier_id, master_id,
       total_tyiyn, comment, reversal_of, user_id, device_id, client_time, created_at)
sale_lines (id, sale_id, line_no, kind piece|container|pour|service, product_id, service_id,
            qty, unit_price_tyiyn, list_price_tyiyn, amount_tyiyn, cost_tyiyn, master_fee_tyiyn)
sale_payments (id, sale_id, method cash|card|transfer|debt|bonus|battery, amount_tyiyn, fee_tyiyn, grams?)  -- grams — аккумулятор в оплату (SPEC-27)

stock_movements (id, branch_id, product_id, qty_delta, value_delta_tyiyn, doc_type, doc_id, created_at)
settings (branch_id, key, value jsonb)
```

### Единицы товара

- `unit = piece`: количество в штуках, цена продажи за штуку.
- `unit = ml`: масло. Количество в миллилитрах, `container_ml` — объём канистры. `sale_price_tyiyn` — цена канистры, единственная цена масла; цена литра на розлив считается от неё (`domain::money::canister_pour_price`, ADR-074), колонка `pour_price_per_l_tyiyn` пуста и запрещена проверкой.
- Показ остатка масла: `stock_qty / container_ml` целых канистр и остаток в литрах.

### Остатки

`branch_products.stock_qty` и `stock_value_tyiyn` — кэш, который меняется только в той же транзакции, что и запись в `stock_movements`. Проверка: сумма движений равна кэшу (инвариант 6).

## Клиент

- Одностраничное приложение, маршруты по модулям: касса, склад, приход, справочники, настройки.
- Состояние сервера получается запросами; локальный кэш и офлайн-очередь — этап 2.
- Расчёт суммы чека на клиенте только для показа; итог определяет сервер. Округление на клиенте повторяет серверное (`web/src/lib/money.ts`, общие тестовые примеры).
- Телефон: нижнее меню (Касса, Чеки, Остатки, Приход, Ещё); таблицы на узком экране показываются карточками (компонент `Table` + `index.css`).
