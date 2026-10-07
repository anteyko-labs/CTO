-- Кассы, движения наличных и смена (SPEC-05, ADR-018, ADR-021, ADR-024, ADR-037).

create table cash_accounts (
    id            uuid primary key,
    branch_id     uuid not null references branches,
    name          text not null,
    kind          text not null check (kind in ('register', 'bank', 'safe', 'other')),
    owner_only    boolean not null default false,
    is_default    boolean not null default false,
    active        boolean not null default true,
    -- Кэш остатка: меняется только вместе с записью в cash_movements.
    balance_tyiyn bigint not null default 0,
    created_at    timestamptz not null default now()
);
create index cash_accounts_branch_idx on cash_accounts (branch_id, active);

create table shifts (
    id                     uuid primary key,
    branch_id              uuid not null references branches,
    number                 bigint not null,
    business_date          date not null,
    account_id             uuid not null references cash_accounts,
    cashier_employee_id    uuid not null references employees,
    opened_by              uuid not null references users,
    opened_device          uuid not null,
    opened_at              timestamptz not null default now(),
    opening_expected_tyiyn bigint not null,
    opening_counted_tyiyn  bigint,
    unique (branch_id, account_id, business_date),
    unique (branch_id, number)
);

create table shift_closes (
    id             uuid primary key,
    shift_id       uuid not null references shifts,
    seq            integer not null,
    expected_tyiyn bigint not null,
    counted_tyiyn  bigint not null,
    diff_tyiyn     bigint not null,
    comment        text not null default '',
    user_id        uuid not null references users,
    device_id      uuid not null,
    closed_at      timestamptz not null default now(),
    unique (shift_id, seq)
);

create table shift_reopens (
    id         uuid primary key,
    shift_id   uuid not null references shifts,
    close_id   uuid not null references shift_closes,
    reason     text not null,
    user_id    uuid not null references users,
    device_id  uuid not null,
    created_at timestamptz not null default now()
);

create table cash_transfers (
    id              uuid primary key,
    branch_id       uuid not null references branches,
    number          bigint not null,
    from_account_id uuid not null references cash_accounts,
    to_account_id   uuid not null references cash_accounts,
    amount_tyiyn    bigint not null check (amount_tyiyn > 0),
    comment         text not null default '',
    reversal_of     uuid references cash_transfers,
    user_id         uuid not null references users,
    device_id       uuid not null,
    created_at      timestamptz not null default now()
);

create table cash_movements (
    id           uuid primary key,
    branch_id    uuid not null references branches,
    account_id   uuid not null references cash_accounts,
    shift_id     uuid references shifts,
    kind         text not null check (kind in ('sale', 'sale_return', 'cash_in', 'cash_out',
                                               'transfer_in', 'transfer_out', 'bank_fee',
                                               'expense', 'payout', 'supplier_payment',
                                               'debt_repayment', 'count_diff', 'reversal')),
    amount_tyiyn bigint not null,
    doc_type     text not null default '',
    doc_id       uuid,
    comment      text not null default '',
    user_id      uuid not null references users,
    device_id    uuid not null,
    created_at   timestamptz not null default now()
);
create index cash_movements_account_idx on cash_movements (account_id, created_at desc);
create index cash_movements_shift_idx on cash_movements (shift_id);

create trigger cash_movements_immutable before update or delete on cash_movements
    for each row execute function forbid_change();
create trigger cash_transfers_immutable before update or delete on cash_transfers
    for each row execute function forbid_change();
create trigger shifts_immutable before update or delete on shifts
    for each row execute function forbid_change();
create trigger shift_closes_immutable before update or delete on shift_closes
    for each row execute function forbid_change();

-- Комиссия банка фиксируется в платеже, как себестоимость в строке (ADR-022).
create table payment_method_fees (
    branch_id   uuid not null references branches,
    method      text not null check (method in ('card', 'transfer')),
    rate_bp     integer not null check (rate_bp >= 0),
    primary key (branch_id, method)
);
alter table sale_payments add column fee_tyiyn bigint not null default 0;

-- Учётный день чека (SPEC-08): у старых чеков — день проведения.
alter table sales add column business_date date;
-- Разовое заполнение нового поля: защита от правки снимается только внутри миграции.
alter table sales disable trigger sales_immutable;
update sales set business_date = (created_at at time zone 'Asia/Bishkek')::date;
alter table sales enable trigger sales_immutable;
alter table sales alter column business_date set not null;
create index sales_business_date_idx on sales (branch_id, business_date);

-- Четыре кассы и ставка банка 0,5 % на каждый филиал.
do $$
declare b record;
begin
    for b in select id from branches loop
        insert into cash_accounts (id, branch_id, name, kind, owner_only, is_default) values
            (gen_random_uuid(), b.id, 'Касса', 'register', false, true),
            (gen_random_uuid(), b.id, 'Счёт', 'bank', false, false),
            (gen_random_uuid(), b.id, 'Сейф магазина', 'safe', false, false),
            (gen_random_uuid(), b.id, 'Сейф владельца', 'safe', true, false);
        insert into payment_method_fees (branch_id, method, rate_bp) values (b.id, 'card', 50), (b.id, 'transfer', 50);
    end loop;
end $$;

-- Наличные и безналичные прошлых чеков переносятся в кассы, чтобы остаток сходился с историей.
insert into cash_movements (id, branch_id, account_id, kind, amount_tyiyn, doc_type, doc_id, user_id, device_id, created_at)
select gen_random_uuid(), s.branch_id,
       (select a.id from cash_accounts a
         where a.branch_id = s.branch_id and a.kind = case when p.method = 'cash' then 'register' else 'bank' end
         limit 1),
       case when s.kind = 'return' then 'sale_return' else 'sale' end,
       p.amount_tyiyn, 'sale', s.id, s.user_id, s.device_id, s.created_at
from sale_payments p join sales s on s.id = p.sale_id
where p.method in ('cash', 'card', 'transfer');

update cash_accounts a set balance_tyiyn = coalesce(
    (select sum(m.amount_tyiyn) from cash_movements m where m.account_id = a.id), 0);
