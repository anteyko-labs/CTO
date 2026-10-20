-- Проверка 20.10.2026 (ADR-088).

-- Что каждое погашение долга и каждое прощение сняло с ожидающего процента кассира (ADR-035):
-- сторно записи журнала долга возвращает ровно это, а не «последние чеки по очереди».
-- Строки не меняются; сторно добавляет строки с обратным знаком.
create table payroll_pending_takes (
    id uuid primary key,
    branch_id uuid not null references branches(id),
    pending_id uuid not null references payroll_pending(id),
    ledger_id uuid not null references party_ledger(id),
    take_tyiyn bigint not null,
    base_tyiyn bigint not null default 0,
    accrued_tyiyn bigint not null default 0,
    created_at timestamptz not null default now()
);
create index payroll_pending_takes_ledger_idx on payroll_pending_takes (ledger_id);
create trigger payroll_pending_takes_immutable before update or delete on payroll_pending_takes
    for each row execute function forbid_change();

-- Выборки по документу: список приходов и карточка поставщика ищут записи долга накладной,
-- исправления и сторно — движения склада документа.
create index if not exists party_ledger_doc_idx on party_ledger (doc_id) where doc_id is not null;
create index if not exists stock_movements_doc_idx on stock_movements (doc_id);

-- Сдачу кассы можно сторнировать и сдать заново (ADR-088): живая сдача у закрытия одна
-- (проверяется под блокировкой кассы), сторнированные остаются в истории.
alter table shift_handovers drop constraint shift_handovers_close_id_key;
create index shift_handovers_close_idx on shift_handovers (close_id);
