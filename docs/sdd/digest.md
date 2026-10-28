# Digest — карта проекта (читать первым)

Avtodom — учёт точки замены масла (Бишкек): касса, склад, долги, книжка ТО, сотрудники и выплаты, прибыль, телеграм-бот, несколько филиалов.

## Стек и каталоги
- `server/` — Rust (axum, sqlx, PostgreSQL 16). Маршруты `server/src/api/*.rs` (52 модуля), права `server/src/auth.rs`, предметные правила `server/src/domain/`, тесты `server/tests/*.rs`.
- `web/` — React + TypeScript (Vite, PWA). Экраны `web/src/pages/`, общие `web/src/components/`, логика с тестами `web/src/lib/*.ts`, e2e `web/e2e/*.spec.ts`.
- `deploy/` — Windows-касса, установка. Выкат сейчас — Mac, порт 8080 (команда — в локальном файле состояния, вне git).

## Где правда
- Инварианты — `docs/sdd/tier0-invariants.md` (полный текст `docs/tier-0/invariants.md`).
- Контракты — `docs/sdd/tier2-contracts.md`.
- Спецификации функций — `docs/tier-4/spec-NN-*.md` (до SPEC-39); новые задачи — `docs/sdd/tasks/NNN-*.md`.
- Решения — `docs/tier-5/decisions.md` (до ADR-135); целиком не читать — `grep -n "^## ADR-NNN"` и `sed -n`.
- Навигация — `docs/nav/NAV.md` (тема → спеки/ADR/файлы), `docs/nav/registry.json` (`jq`; обновлять `python3 scripts/registry.py`).

## Основные области
- Касса: `pages/Cashier.tsx`, `api/sales.rs`; оплата, долг, офлайн-очередь; изменение оплаты — `api/pay_fix.rs` (SPEC-37).
- Смена и отчёт: `api/shift_report.rs`, `api/cash.rs`.
- Склад: приход `api/receipts.rs` (черновики по фото — `api/receipt_drafts.rs`), приёмка на планшете `api/acceptances.rs` (SPEC-39), ревизия `api/revisions.rs`, перелив.
- Долги: `party_ledger`, `api/parties.rs`, старый долг (SPEC-38), сверка.
- Книжка ТО: `lib/oilBook.ts`, `lib/nextChange.ts`, `lib/oilBookPrint.ts`, напоминания (SPEC-32).
- Бот: `api/telegram.rs`, `api/bot_*.rs`; уведомления `api/notifications.rs`.
- Банк Optima: QR и опрос, `api/optima.rs` (SPEC-31).

## Текущее
- main 6abc16d, выкачено на 8080; незакрытые хвосты — `docs/sdd/tasks/000-in-progress.md`.
