---
type: doc
status: ready
tier: 3
updated: 2026-10-08
---

# Путеводитель разработчика

Документ для того, кто впервые открывает код: где что лежит, как устроена любая операция, как добавить функцию и обо что легко споткнуться. Правила и решения здесь не повторяются, а даются ссылками — источник правды остаётся в `docs/tier-0`…`tier-5`.

## 1. Порядок знакомства

1. [README.md](../README.md) — что за продукт и статус этапов.
2. [tier-0/invariants.md](tier-0/invariants.md) — 20 правил, которые нельзя нарушать. При любом сомнении решает этот файл.
3. [tier-1/CHECKLIST.md](tier-1/CHECKLIST.md) — что сделано и что следующее.
4. [tier-3/arch-core.md](tier-3/arch-core.md) — модель данных, соглашения API, роли.
5. Спецификация нужной функции в [tier-4/](tier-4/) и связанные ADR в [tier-5/decisions.md](tier-5/decisions.md).

## 2. Карта кода

Каждая функция описана одной спецификацией. По номеру SPEC легко найти и код, и тесты.

| SPEC | О чём | Сервер (`server/src/`) | Клиент (`web/src/`) |
| --- | --- | --- | --- |
| 01 | Вход, роли, журнал, идемпотентность | `auth.rs`, `api/auth.rs`, `ops.rs`, `bootstrap.rs` | `pages/Login.tsx`, `pages/Users.tsx`, `lib/auth.tsx`, `lib/api.ts` |
| 02 | Каталог, штрихкоды, этикетки, справочники | `api/catalog.rs`, `api/refs.rs`, `domain/barcode.rs` | `pages/Products.tsx`, `Categories.tsx`, `Employees.tsx`, `Services.tsx`, `Suppliers.tsx`, `LabelSettings.tsx`, `components/ProductFormModal.tsx`, `ProductPicker.tsx`, `UnknownCodeModal.tsx`, `labels.ts` |
| 03 | Приход и остатки | `api/receipts.rs`, `domain/costing.rs` | `pages/Receipts.tsx`, `ReceiptNew.tsx`, `ReceiptView.tsx`, `Stock.tsx` |
| 04 | Касса: продажа и возврат | `api/sales.rs`, `domain/money.rs` | `pages/Cashier.tsx`, `SalesDay.tsx`, `SaleView.tsx`, `components/salePrint.ts`, `ServicePicker.tsx`, `lib/money.ts` |
| 05 | Смена, кассы, наличные | `api/cash.rs` | `pages/Shift.tsx` |
| 06 | Операционные расходы | `api/expenses.rs` | `pages/Expenses.tsx`, `components/QuickExpense.tsx` |
| 07 | Начисления и выплаты сотрудникам | `api/payroll.rs` | `pages/Payroll.tsx` |
| 08 | Прибыль, сводка владельца | `api/reports.rs` | `pages/Owner.tsx` |
| 09 | Касса без сети | `api/offline.rs` | `lib/offline.ts`, `components/OfflineBar.tsx` |
| 10 | Контрагенты, долги, балансы | `api/parties.rs` | `pages/Clients.tsx`, `ClientCard.tsx`, `Debts.tsx`, `SupplierCard.tsx`, `components/ClientPicker.tsx` |
| 11 | Подарки в чеке | `api/gifts.rs` | `pages/Gifts.tsx`, `components/GiftPicker.tsx` |
| 12 | Кабинет юрлица по ИНН | `api/cabinet.rs` | `pages/cabinet/CabinetApp.tsx` |
| 13 | Уведомления владельцу | `api/notifications.rs` | `pages/Notifications.tsx` |
| 14 | Перелив масла | `api/oil.rs` | `pages/OilTransfer.tsx` |
| 15 | Ревизия склада | `api/revisions.rs` | `pages/Revision.tsx` |
| 16 | Масляная книжка | `api/oil_book.rs` | `components/OilBook.tsx` |
| 18 | Телеграм-бот владельца | `api/telegram.rs` | `pages/TelegramSettings.tsx` |
| 17 | Аналоги фильтров | `api/analogs.rs` | `components/ProductFormModal.tsx`, `pages/Cashier.tsx` |
| — | Аккумуляторы на вес (ADR-048) | `api/batteries.rs` | `components/BatteryIntake.tsx`, `ServicePicker.tsx` |

Остальное:

- `server/migrations/NNNN_*.sql` — схема базы, по одному файлу на шаг. Применяются при старте сервера.
- `server/.sqlx/` — сохранённые описания SQL-запросов, по ним сервер собирается без базы (CI, Docker).
- `server/tests/flow.rs` — сценарии учёта на реальной базе. `server/tests/http.rs` — API, права, вход.
- `web/e2e/` — сквозные тесты в браузере. `web/src/lib/*.test.ts` — модульные тесты клиента.
- `deploy/`, `Dockerfile` — развёртывание. `.githooks/` — проверки перед коммитом. `.github/workflows/ci.yml` — проверки на GitHub.

## 3. Как устроена изменяющая операция

Любая операция, которая меняет учёт (чек, приход, выплата, расход, долг), собрана по одному шаблону из `server/src/ops.rs`. Новый код должен следовать ему же:

```rust
pub async fn post_x_tx(conn: &mut PgConnection, ctx: &Ctx, req: XReq) -> AppResult<XOut> {
    const KIND: &str = "x.post";
    // 1. Повтор того же op_id возвращает сохранённый ответ (инвариант 8, ADR-013).
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    // 2. Проверки входа → AppError::Validation (422), права → AppError::Forbidden (403).
    // 3. Блокировка строк остатков в порядке id: receipts::lock_products(...).
    // 4. Номер документа: ops::next_counter(conn, branch_id, "x").
    // 5. Запись документа и строк. Движения склада — только через ops::apply_movement.
    // 6. Журнал: ops::audit(conn, ctx, KIND, "x", Some(id), json!({...})).
    let out = load_x(conn, &ctx.user, id).await?;
    // 7. Ответ сохраняется в той же транзакции.
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}
```

Обработчик HTTP только открывает транзакцию, вызывает `*_tx` и делает `commit`. Тесты в `server/tests/flow.rs` вызывают `*_tx` напрямую.

Что обеспечивает база, а не код:

- Таблицы проведённых документов, движений и журнала защищены триггером `forbid_change`: `UPDATE` и `DELETE` отклоняются (ADR-014). Исправление делается только новым сторнирующим документом.
- Уникальные индексы защищают от двойного сторно и дублей штрихкодов.

Роли:

- Извлекатели в `auth.rs`: `CurrentUser` — любой вошедший, `Ctx` — пользователь и `X-Device-Id` для изменяющих запросов, `Owner` — только владелец.
- Поля для владельца (себестоимость, стоимость остатка, прибыль) сервер не кладёт в ответ администратору: `Option` + `skip_serializing_if`. Скрыть кнопку в интерфейсе недостаточно (инвариант 12).

Деньги и количества:

- Только `i64`, расчёты с делением идут через `domain::money::div_round`. Это единственное место округления.
- Клиентская копия правила в `web/src/lib/money.ts` служит только для показа и проверяется теми же примерами.
- Ввод пользователя разбирается `parseSom` / `parseLiters` из `web/src/lib/format.ts`, без `parseFloat`.

## 4. Как добавить функцию

1. Спецификация `docs/tier-4/spec-NN-*.md`: задача, модель данных, алгоритм, права, экраны, критерии приёмки. Без неё код не пишется (инвариант 20).
2. Если нужна новая библиотека, сервис или отступление от правила — новая запись ADR в `docs/tier-5/decisions.md`. Старые записи не правятся.
3. Миграция — новый файл `server/migrations/NNNN_*.sql`. Уже применённые миграции не меняются никогда.
4. Сервер: модуль `server/src/api/<тема>.rs` с заголовком `//! … (SPEC-NN)`, маршруты подключаются в `api/mod.rs`. Изменяющие операции строятся по шаблону из раздела 3.
5. Тесты по критериям приёмки: сценарии в `tests/flow.rs`, права и HTTP в `tests/http.rs`.
6. `cargo sqlx prepare -- --all-targets` — обновить `server/.sqlx/` и закоммитить его вместе с кодом.
7. Клиент: типы ответов в `web/src/lib/types.ts`, экран в `web/src/pages/`, маршрут и пункт меню в `App.tsx`. Поведение интерфейса — по [tier-3/ui-rules.md](tier-3/ui-rules.md).
8. Если меняется путь пользователя на кассе или складе — дополнить `web/e2e/`.
9. Отметить пункт в `docs/tier-1/CHECKLIST.md`, обновить статус в README. Документация коммитится в том же коммите, что и код.

## 5. Проверки

| Где | Команда | Что проверяет |
| --- | --- | --- |
| Сервер | `cd server && cargo test` | Модульные и интеграционные тесты. Каждый тест создаёт свою временную базу, нужен `DATABASE_URL` в `server/.env` |
| Сервер | `cargo clippy --all-targets -- -D warnings` | В том числе запрет `unwrap`/`expect`/`panic` вне тестов (инвариант 18) |
| Клиент | `cd web && npx tsc -b && npx oxlint src && npm test && npm run build` | Типы, lint, модульные тесты, сборка |
| Браузер | `cd web && npm run e2e` | Сквозной путь: товар → приход → продажа → возврат → остатки, права администратора. Нужен запущенный сервер с владельцем `owner` / `owner-pass-1` |
| Перед коммитом | `.githooks/pre-commit`, `commit-msg` | Формат и clippy для `server/`, секреты, файлы окружения, длина заголовка коммита |
| GitHub | `.github/workflows/ci.yml` | Всё перечисленное на каждый push, включая браузерные тесты |

Хуки включаются после клонирования: `git config core.hooksPath .githooks`.

## 6. Подводные камни

- **Windows: занятый бинарник.** Пока работает `avtodom-server.exe`, `cargo build` и `cargo test` не могут его перезаписать (ошибка «Отказано в доступе»). Сначала остановите сервер.
- **`.env` не загружается сервером.** `cargo run` читает только переменные окружения, а `server/.env` используют лишь SQLx при сборке и тесты. Перед запуском выполните `set -a && . ./.env && set +a`.
- **Порт 8080 занят.** Это значит, что сервер уже запущен в другом окне: второй экземпляр не стартует.
- **Хуки и PATH.** Если `cargo` не найден в PATH, pre-commit пропускает проверки Rust с предупреждением. Это не ошибка, но проверки тогда не выполнены.
- **Изменили SQL и не обновили `.sqlx`.** Локально всё собирается (там есть база), а в CI и Docker сборка падает. Лечится через `cargo sqlx prepare -- --all-targets`.
- **Браузер для e2e.** Если Chromium не скачивается, можно взять установленный Edge: `E2E_CHANNEL=msedge npm run e2e`.
- **Блокировка входа.** После 5 неверных паролей логин закрыт на 15 минут (ADR-016). Это касается и тестов, которые подбирают пароль.
- **Офлайн-касса.** Чек всегда сначала кладётся в очередь на устройстве (`lib/offline.ts`) и только потом уходит на сервер. Касса не должна обходить очередь.
- **Часовой пояс.** В базе время хранится в UTC, «день» считается по Asia/Bishkek: в SQL — `at time zone 'Asia/Bishkek'`, в клиенте — `todayBishkek()`.

## 7. Словарь

| Термин | Значение |
| --- | --- |
| Тыйын | 1/100 сома. Все суммы хранятся в тыйынах |
| Приход | Приходная накладная от поставщика, увеличивает остаток и его стоимость |
| Сторно | Документ-отмена со ссылкой на исходный (`reversal_of`). Исходный документ не меняется |
| Движение склада | Строка `stock_movements`. Сумма движений равна остатку (инвариант 6) |
| Пул стоимости | Количество и стоимость остатка товара. Себестоимость продажи берётся из него пропорционально (ADR-011) |
| Розлив | Продажа масла в миллилитрах из открытой канистры по цене за литр |
| Перелив | Перенос масла между товарами. Делает только владелец (этап 4) |
| Смена | Рабочий день кассы от открытия до закрытия с пересчётом наличных (SPEC-05) |
| Контрагент | Клиент или поставщик со своим балансом долга (SPEC-10) |
| Филиал | Точка. Все данные учёта привязаны к `branch_id`, пока филиал один |
| `op_id` | Идентификатор операции от клиента. Повтор с тем же `op_id` не создаёт дубль |
