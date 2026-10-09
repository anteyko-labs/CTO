-- Аккумуляторы на вес (ADR-048) и кабинет юрлица по ИНН (ADR-049).

-- Товар на вес: остаток в граммах, цена — за килограмм.
alter table products drop constraint products_unit_check;
alter table products add constraint products_unit_check check (unit in ('piece', 'ml', 'g'));
alter table sale_lines drop constraint sale_lines_kind_check;
alter table sale_lines add constraint sale_lines_kind_check
    check (kind in ('piece', 'container', 'pour', 'service', 'weight'));
alter table cash_movements drop constraint cash_movements_kind_check;
alter table cash_movements add constraint cash_movements_kind_check
    check (kind in ('sale', 'sale_return', 'cash_in', 'cash_out', 'transfer_in', 'transfer_out',
                    'bank_fee', 'expense', 'payout', 'supplier_payment', 'debt_repayment',
                    'count_diff', 'reversal', 'battery_intake'));

-- Приём старых аккумуляторов: вес, цена за кг, сколько выдали из кассы.
create table battery_intakes (
    id                 uuid primary key,
    branch_id          uuid not null references branches,
    number             bigint not null,
    product_id         uuid not null references products,
    grams              bigint not null check (grams > 0),
    price_per_kg_tyiyn bigint not null check (price_per_kg_tyiyn >= 0),
    amount_tyiyn       bigint not null check (amount_tyiyn >= 0),
    account_id         uuid not null references cash_accounts,
    comment            text not null default '',
    user_id            uuid not null references users,
    device_id          uuid not null,
    created_at         timestamptz not null default now(),
    unique (branch_id, number)
);
create trigger battery_intakes_immutable before update or delete on battery_intakes
    for each row execute function forbid_change();

-- Кабинет юрлица: пароль, смена при первом входе, свои сессии.
create table party_accounts (
    party_id      uuid primary key references parties,
    password_hash text not null,
    must_change   boolean not null default true,
    last_login_at timestamptz,
    updated_at    timestamptz not null default now()
);

create table client_sessions (
    token_hash bytea primary key,
    party_id   uuid not null references parties,
    expires_at timestamptz not null,
    created_at timestamptz not null default now()
);
create index client_sessions_party_idx on client_sessions (party_id);
