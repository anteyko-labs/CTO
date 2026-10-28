---
type: doc
status: ready
tier: 3
updated: 2026-10-28
---

# Навигатор по проекту

Карта «что где» по темам. Подробности со строками — в `docs/nav/registry.json` (собирает `python3 scripts/registry.py`, проверка свежести — `--check`; в CI падает, в pre-commit — предупреждение).

## Как пользоваться

1. Найти тему ниже: SPEC, ADR, файлы, таблицы, тесты.
2. Запросом `jq` к реестру получить файл и строки (функция, маршрут, раздел SPEC, границы ADR).
3. Читать только эти строки (`sed -n '120,160p' файл` или Read с offset/limit), а не файл целиком.

В реестре: `spec` (разделы со строками, ADR, пути кода), `adr` (start/end, статус, связи), `routes` (метод, путь, обработчик, право), `web_routes`, `server_files` (`fns`), `web_files` (`exports`, `types`), `migrations`, `tables`, `tests`.

```sh
R=docs/nav/registry.json
# ADR по номеру → строки в decisions.md
jq -c '.adr[]|select(.id=="ADR-084")|{title,start,end,status,relations,touched_by}' $R
# маршрут API по части пути: метод, файл:строка, обработчик, право
jq -r '.routes[]|select(.path|test("shifts/\\{id\\}/report"))|"\(.method) \(.path) \(.file):\(.handler_line) \(.permission // "")"' $R
# функция сервера по имени
jq -r '.server_files[]|.file as $f|.fns[]|select(.name=="apply_movement")|"\($f):\(.line)"' $R
# функция или компонент клиента по имени
jq -r '.web_files[]|.file as $f|.exports[]|select(.name=="useScanToPicker")|"\($f):\(.line)"' $R
# тесты по слову (сервер, e2e, модульные)
jq -r '.tests[]|.file as $f|.cases[]|select(.name|test("смен";"i"))|"\($f):\(.line) \(.name)"' $R
# какая миграция создала и меняла таблицу
jq -c '.tables[]|select(.table=="party_vehicles")' $R
# разделы SPEC со строками
jq -r '.spec[]|select(.id=="SPEC-34")|.file, (.sections[]|"\(.line) \(.title)")' $R
# все ADR, где упомянут SPEC
jq -r '.adr[]|select(.spec|index("SPEC-33"))|"\(.id) \(.title)"' $R
```

## Темы

### Касса и режим кассы
- SPEC-04, SPEC-29 («Режим кассы»), SPEC-30 §2–4 (чек «В сервис»), SPEC-36 §2. ADR-071 (несколько смен, кассир смены), ADR-096 (ставка мастеру по услуге), ADR-100 (чек «В сервис» с клиентом), ADR-101 (экранная клавиатура), ADR-105 (возврат без чека — владелец), ADR-114 (Умага на кассе нет).
- `server/src/api/sales.rs` (`post_sale_tx`, возврат), `web/src/pages/Cashier.tsx` (2900 строк — только по реестру), `web/src/lib/serviceSale.ts`, `web/src/lib/cashierMode.ts`, `web/src/components/Keypad.tsx`, `CashierModeBar.tsx`, `CashierFavorites.tsx`.
- Таблицы: `sales`, `sale_lines`, `sale_payments`, `cancelled_lines`. Тесты: `server/tests/flow.rs`, e2e `cashier-mode`, `cashier-undo`, `stage1`, `point-feedback-4-kassa`.

### Сканер и фокус
- ui-rules §4, SPEC-30 §13, ADR-026 (штрихкод обязателен). Сканер — та же клавиатура: код идёт туда, где фокус, и жмёт Enter.
- `web/src/lib/scanner.ts` (`useScanToPicker`, `SCAN_GAP_MS`), `web/src/lib/focus.ts`, `web/src/components/ProductPicker.tsx`, `web/src/lib/pickerResults.ts`, `UnknownCodeModal.tsx`.
- Тесты: e2e `scanner-focus` (18 сценариев), `picker-enter`.

### Книжка и следующая замена
- SPEC-16, SPEC-34, SPEC-36 §1, §3. ADR-072 (строки книжки по фильтрам), ADR-084 (мили), ADR-104 (бесплатная замена с нашим маслом), ADR-109/111 (следующая замена по услугам этого чека), ADR-115 (только пробег), ADR-116/117 (чек книжки, доливка не замена).
- `server/src/api/oil_book.rs` (`record_from_sale`, `resolve_target`, `sale_book`), `server/src/domain/oil_service.rs`, `server/src/domain/mileage.rs`, `web/src/lib/nextChange.ts`, `web/src/components/OilBook.tsx`, `web/src/lib/mileage.ts`.
- Таблицы: `oil_changes`, `oil_change_items`, `vehicle_book_intervals`, `oil_reminders`, `party_vehicles`. Тесты: e2e `next-change*`, `web/src/lib/nextChange.test.ts`, `mileage.test.ts`.

### Чек и печать
- SPEC-04, SPEC-36 §4, ADR-091 (печать по проверке), ADR-116 (книжка в печатном чеке), ADR-127 (печать без окна, `--kiosk-printing`), ADR-010 (этикетки).
- `web/src/components/salePrint.ts` (`bookHtml`), `web/src/lib/printFrame.ts` (`printInFrame`), `web/src/lib/oilBookPrint.ts`, `web/src/components/labels.ts`, `web/src/pages/SaleView.tsx`.
- Тесты: `web/src/components/salePrint.test.ts`.

### Смена и отчёт смены
- SPEC-05, SPEC-30 §7–8. ADR-020/064 (недостача с кассира), ADR-071, ADR-081 (PIN кассира, сдача кассы), ADR-120 (чистая прибыль, безнал по счетам), ADR-121 (отчёт смены в ленту). Черновик SPEC-37, ADR-132 — «Изменить оплату» чека и «из наличных в QR» за смену, поправка закрытой смены и удержания.
- `server/src/api/cash.rs` (`close_shift`), `server/src/api/shift_report.rs`, `web/src/pages/Shift.tsx`, `web/src/pages/ShiftReport.tsx`, `web/src/components/ShiftOpen.tsx`, `web/src/lib/shiftReport.ts`.
- Таблицы: `shifts`, `shift_closes`, `shift_reopens`, `shift_handovers`. Тесты: `server/tests/shift_report.rs`, e2e `shift-report`.

### Деньги, кассы, счета, банк и QR
- SPEC-05, SPEC-08, SPEC-27 §6, SPEC-28 §5, SPEC-31. ADR-018/037 (кассы, сейфы), ADR-022/046/077 (комиссия у счёта), ADR-076 (свои счета), ADR-088 (точные сторно), ADR-106 (Optima: QR и сверка). Черновик SPEC-37 (изменение оплаты, перенос в банк).
- `server/src/api/cash.rs`, `income.rs`, `fees.rs`, `reports.rs` (прибыль), `optima.rs`, `bank_qr.rs`, `bank_recon.rs`; клиент `web/src/pages/Accounts.tsx`, `Bank.tsx`, `BankRecon.tsx`, `web/src/lib/accounts.ts`, `web/src/components/BankQrPanel.tsx`.
- Таблицы: `cash_accounts`, `cash_movements`, `cash_transfers`, `cash_account_fees`, `bank_qr`, `bank_operations`, `bank_fee_adjustments`, `expenses`. Тесты: `server/tests/optima.rs`, e2e `accounts`, `optima-qr`.

### Долги и юрлица
- SPEC-10, SPEC-12, SPEC-27 §5–7. ADR-030 (юрлицо), ADR-035 (процент по мере погашения), ADR-049 (кабинет по ИНН), ADR-073 (оплата по частям), ADR-095 («По счёту»). Черновик SPEC-38, ADR-133 — старый долг из тетради с датой долга.
- `server/src/api/parties.rs`, `supplier_debts.rs`, `cabinet.rs`, `bot_debt.rs`, `server/src/domain/debt.rs`; клиент `pages/ClientCard.tsx`, `Debts.tsx`, `SupplierCard.tsx`, `components/RepaymentModal.tsx`, `lib/debtDocs.ts`, `pages/cabinet/CabinetApp.tsx`.
- Таблицы: `parties`, `party_ledger`, `party_balances`, `party_accounts`, `supplier_allocations`, `debt_reminders`. Тесты: `server/tests/invoice.rs`, `flow.rs`.

### Зарплата и выплаты
- SPEC-07, SPEC-23. ADR-019, ADR-036 (2 % с валовой), ADR-065 (без баллов), ADR-112 (мастеру 30 сом за услугу), ADR-122 (выплаты по видам, аванс; кассиру — только владелец).
- `server/src/api/payroll.rs`, `server/src/api/bot_staff.rs`; клиент `pages/Payroll.tsx`, `components/PayrollForms.tsx`, `PayrollMonth.tsx`, `PayrollEntries.tsx`.
- Таблицы: `payroll_accruals`, `payouts`, `employee_pay_rules`, `payroll_pending`, `payroll_cost_pending`.

### Склад: приход, ревизия, перелив, брак
- SPEC-03, SPEC-14, SPEC-15, SPEC-20, SPEC-25, SPEC-26, SPEC-29 §8–9, SPEC-30 §9, §11. ADR-011/044/087 (пул стоимости, сторно), ADR-069 (брак), ADR-074/075 (цена масла, цена в приходе), ADR-086 (оприходование), ADR-098 (накладная по фото), ADR-123 (канистры и литры), ADR-125 («Склад → Остатки»). Черновик SPEC-39, ADR-134 — приёмка на планшете с PIN и подписью кассира.
- `server/src/ops.rs` (`apply_movement` — единственный путь движения), `server/src/domain/costing.rs`, `server/src/api/receipts.rs`, `stock_in.rs`, `revisions.rs`, `oil.rs`, `defects.rs`, `supplier_returns.rs`, `receipt_drafts.rs`; клиент `pages/ReceiptNew.tsx`, `Revision.tsx`, `OilTransfer.tsx`, `Products.tsx`, `lib/units.ts`.
- Таблицы: `stock_movements`, `branch_products`, `receipts`, `receipt_lines`, `stock_ins`, `revisions`, `oil_transfers`, `defects`, `supplier_returns`. Тесты: `flow.rs`, e2e `stock-in`, `receipt-price`, `receipt-photo`.

### Умаг (старая база)
- SPEC-20, SPEC-35. ADR-056, ADR-110 (коды Умага — коды товара), ADR-113 (правило кодов), ADR-114, ADR-126 (выключатель).
- `server/src/api/old_catalog.rs` (`link_row`, `auto_link`, `pairs`), `server/src/api/catalog.rs` (`product_by_barcode`), миграции 0027, 0065, 0067; клиент `lib/umagCodes.ts` (то же правило, что SQL), `lib/umagLink.ts`, `components/Umag.tsx`, `pages/UmagLink.tsx`.
- Таблицы: `old_catalog`, `old_catalog_link_rejects`, `product_barcodes`. Тесты: `server/tests/umag.rs`, e2e `umag-codes`.

### Бот: клиенты, работники, владелец
- SPEC-18, SPEC-19, SPEC-21…24, SPEC-30 §10. ADR-050, ADR-053/124 (баллы только за замену), ADR-060/067 (вопросы в режиме «Спросить»), ADR-063 (доставка по чатам), ADR-066 (кабинет работника), ADR-068 (черновик → «Провести»), ADR-119 («Сейчас в машине», «Прошлые ТО»), ADR-121 (лента). Черновик SPEC-39 §7, ADR-135 — накладные в бот от администратора.
- `server/src/api/telegram.rs` (4100 строк: команды, лента), `bot_cars.rs`, `bot_book.rs`, `bot_staff.rs`, `bot_drafts.rs`, `bot_reports.rs`, `loyalty.rs`, `notifications.rs`, вопросы — `assistant.rs`.
- Таблицы: `telegram_links`, `telegram_customers`, `telegram_employees`, `telegram_sent`, `bot_state`, `bot_input`, `bot_drafts`, `loyalty_ledger`. Тесты: `server/tests/bot_book.rs`, `flow.rs`.

### Рассылки клиентам
- SPEC-32, ADR-107 (Телеграм и официальный WhatsApp). `server/src/api/client_messages.rs`, `web/src/pages/ClientMessages.tsx`, `web/src/lib/clientMessages.ts`.
- Таблицы: `client_messages`, `client_message_keys`. Тесты: `server/tests/client_messages.rs`, e2e `client-messages`.

### Филиалы
- SPEC-33, ADR-108. `server/src/auth.rs` (`request_branch`, заголовок `X-Branch-Id`), `server/src/api/branches.rs` (`report`, `merge_reports`), `web/src/lib/branch.ts`, `lib/branchReport.ts`, `pages/Branches.tsx`.
- Таблицы: `branches`, `branch_settlements`, `party_balances`. Тесты: `server/tests/branches.rs` (обход всех маршрутов), e2e `z-branches` (идёт последним).

### Права
- SPEC-01, SPEC-30 §5. ADR-005, ADR-082, ADR-097 (права администраторов выбирает владелец), ADR-103. Инвариант: права проверяет сервер, прибыль администратору не отдаётся.
- `server/src/auth.rs` (`PERMISSIONS`, `require_permission`, `require_owner`, `Owner`), `web/src/lib/permissions.ts`, `web/src/pages/Users.tsx`. Права у маршрутов: `jq '.routes[]|select(.permission or .owner_only)'`.
- Таблицы: `users`, `user_permissions`, `sessions`, `login_attempts`. Тесты: `server/tests/http.rs`, e2e `admin-permissions`.

### Офлайн-касса и снимок
- SPEC-09. ADR-013 (идемпотентность по `op_id`), ADR-033, ADR-038, ADR-062, ADR-093 (снимок, очередь, отказы).
- `server/src/api/offline.rs` (`GET /offline/snapshot`, `offline-rejected`, `by-op`), `web/src/lib/offline.ts` (`enqueueSale`, IndexedDB), `web/src/lib/api.ts` (`serverReachable`), `web/src/components/OfflineBar.tsx`.
- Тесты: e2e `offline-queue`, `stage1` («касса работает без сети»).

### Установка на Windows и копии
- ADR-057 (без Docker, Caddy, Tailscale), ADR-089 (откат, хранение копий), ADR-127 (ярлык кассы).
- `deploy/windows/install.ps1`, `update.ps1`, `backup.ps1`, `restore.ps1` (`-Check`), `rollback.ps1`, `deploy/windows/README.md`; `deploy/check-restore.sh` (задача `restore` в CI); пакет — `.github/workflows/windows-package.yml`.

## Ловушки

- **Сканер и фокус.** Сканер печатает туда, где фокус, и жмёт Enter: кнопка с фокусом нажимается повторно, код в поле количества становится количеством. Любое новое поле или кнопка на кассе — через `lib/scanner.ts`/`lib/focus.ts`, прогнать e2e `scanner-focus`.
- **Снимок кассы офлайн.** Чек всегда идёт через очередь `lib/offline.ts`. Новое поле товара, услуги, клиента или машины, нужное кассе, добавляется и в снимок (`server/src/api/offline.rs` → `SnapProduct`, `SnapService`…; клиент `lib/offline.ts` → `SnapshotProduct`), иначе без сети касса работает иначе. Себестоимость в снимок не кладётся.
- **Мили.** Пробег хранится как вписан и в км (`mileage_km`); интервалы книжки — в км. Перевод только `domain::mileage` и `lib/mileage.ts` (одно правило, целые).
- **Порядок миграций.** 0066 влилась в main раньше 0065/0067 (ветка SPEC-35): на рабочей базе 0065 и 0067 применяются после 0066. Миграции не должны зависеть друг от друга сверх номера; применённые не правятся; новый номер — после последнего в main.
- **Схема `public.` в SQL-функциях.** `pg_dump` ставит пустой `search_path`: функция в вычисляемом столбце или индексе зовёт свои функции только как `public.f(...)`, иначе `pg_restore` не создаёт таблицу. Проверка — `deploy/check-restore.sh`.
- **SQLx offline.** После правки SQL — `cargo sqlx prepare -- --all-targets` и коммит `server/.sqlx`; коммитить с `SQLX_OFFLINE=true` (хук запускает clippy).
- **ADR только добавляются.** Изменение решения — новая запись со ссылкой; у старой меняется только статус. Номер ADR — следующий за последним в main.
- **Филиал запроса.** Документы филиала — с `branch_id = $n`, общие справочники — без; новый маршрут ловит `server/tests/branches.rs`.
- **Реестр.** После правки кода, SPEC, ADR, миграций или тестов — `python3 scripts/registry.py`, иначе CI падает.
