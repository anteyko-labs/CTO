-- Строки чека без цены закупки и ожидание процента кассира по ним (ADR-062).

-- Себестоимость строки неизвестна: товар продали, а закупочной цены ещё не было ни в одном
-- приходе. В строке остаётся 0 (инвариант 3: себестоимость фиксируется при проведении),
-- отметка нужна отчёту прибыли и проценту кассира.
alter table sale_lines add column cost_unknown boolean not null default false;

-- 2 % кассира по такой строке ждут, пока владелец не укажет цену закупки.
-- Рабочее состояние, как payroll_pending: строка закрывается отметкой, а не удаляется.
create table payroll_cost_pending (
    id                  uuid primary key,
    branch_id           uuid not null references branches,
    sale_id             uuid not null references sales,
    line_no             integer not null,
    product_id          uuid not null references products,
    employee_id         uuid not null references employees,
    rule_id             uuid references employee_pay_rules,
    -- Ставка, действовавшая в учётный день чека; 0 — правила процента не было.
    rate_bp             integer not null,
    -- Сколько единиц строки не покрыто ни остатком, ни прошлой закупкой.
    unknown_units       bigint not null check (unknown_units > 0),
    resolved_cost_tyiyn bigint,
    resolved_at         timestamptz,
    resolved_by         uuid references users,
    created_at          timestamptz not null default now(),
    unique (sale_id, line_no)
);
create index payroll_cost_pending_open_idx on payroll_cost_pending (branch_id, product_id)
    where resolved_at is null;

-- Откуда оплачена накладная «оплачена сразу» (ADR-062): из кассы (`account`, какая — в
-- `paid_account_id`) или из своих денег владельца мимо касс (`outside`). У старых накладных пусто.
alter table receipts add column paid_from text check (paid_from in ('account', 'outside'));
alter table receipts add column paid_account_id uuid references cash_accounts;
alter table receipts add constraint receipts_paid_account_check
    check ((paid_from = 'account') = (paid_account_id is not null));
