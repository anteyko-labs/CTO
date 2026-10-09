-- Бонусные баллы клиентов в Телеграм-боте (SPEC-19, ADR-053). 1 балл = 1 сом, храним в тыйынах.
create table loyalty_ledger (
    id           uuid primary key,
    branch_id    uuid not null references branches,
    party_id     uuid not null references parties,
    kind         text not null check (kind in ('accrual', 'redeem', 'refund', 'accrual_back')),
    amount_tyiyn bigint not null check (amount_tyiyn <> 0),
    rate_bp      integer,
    doc_type     text not null,
    doc_id       uuid not null,
    created_at   timestamptz not null default now()
);
create index loyalty_ledger_party_idx on loyalty_ledger (party_id, created_at);
create index loyalty_ledger_doc_idx on loyalty_ledger (doc_id);
create trigger loyalty_ledger_immutable before update or delete on loyalty_ledger
    for each row execute function forbid_change();

-- Какие операции с баллами клиенту уже сообщили в бот.
create table loyalty_notices (
    ledger_id uuid primary key references loyalty_ledger,
    sent_at   timestamptz not null default now()
);

-- Оплата баллами — способ оплаты чека без денег, как долг.
alter table sale_payments drop constraint sale_payments_method_check;
alter table sale_payments add constraint sale_payments_method_check
    check (method in ('cash', 'card', 'transfer', 'debt', 'bonus'));
