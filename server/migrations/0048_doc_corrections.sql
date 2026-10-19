-- «Исправить» у документов владельца (SPEC-29 §24, ADR-085): исходный документ сторнируется
-- своей функцией, верный проводится своей — одной операцией. Сами документы не меняются
-- (инвариант 4): связь «исправлен → новый» хранится здесь, запись только добавляется.
create table doc_corrections (
    id              uuid primary key,
    branch_id       uuid not null references branches,
    -- Повтор того же «Сохранить» (обрыв связи) не проводит исправление второй раз.
    op_id           uuid not null unique,
    kind            text not null check (kind in (
                        'sale', 'receipt', 'expense', 'cash_movement', 'cash_transfer', 'ledger',
                        'accrual', 'payout', 'stock_in', 'supplier_return', 'battery_intake',
                        'oil_transfer')),
    -- Документ исправляется один раз: дальше исправляют новый.
    original_id     uuid not null unique,
    original_number bigint,
    reversal_id     uuid not null,
    new_id          uuid not null unique,
    new_number      bigint,
    comment         text not null check (btrim(comment) <> '' and char_length(comment) <= 500),
    -- Что поменялось: [{what, old, new}] — словами, как видел владелец в момент исправления.
    diff            jsonb not null default '[]',
    user_id         uuid not null references users,
    device_id       uuid not null,
    created_at      timestamptz not null default now(),
    check (original_id <> new_id)
);
create index doc_corrections_branch_idx on doc_corrections (branch_id, created_at desc);
create trigger doc_corrections_immutable before update or delete on doc_corrections
    for each row execute function forbid_change();
