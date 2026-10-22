-- Optima Business: QR на сумму чека и сверка с выпиской банка (SPEC-31, ADR-106).

-- QR на сумму чека. Это не проведённый документ, а ожидание оплаты: состояние меняет опрос
-- банка (ждём → оплачен / не оплачен / ошибка) или кассир («Отменить»). Деньги проводит чек,
-- который ссылается на оплаченный QR.
create table bank_qr (
    id             uuid primary key,
    branch_id      uuid not null references branches,
    -- Повтор того же запроса кассы — тот же QR.
    op_id          uuid not null unique,
    user_id        uuid not null references users,
    device_id      uuid not null,
    amount_tyiyn   bigint not null check (amount_tyiyn > 0),
    -- Номер транзакции банка из ответа генерации QR.
    transaction_id text not null,
    qr_base64      text not null default '',
    qr_url         text not null default '',
    expires_at     timestamptz not null,
    status         text not null default 'waiting'
                   check (status in ('waiting', 'paid', 'unpaid', 'error', 'cancelled')),
    -- Ответ банка об оплате: сумма, номер операции, время.
    paid_tyiyn     bigint,
    operation_id   text,
    paid_at        timestamptz,
    -- Чек, который провёл эту оплату; один QR — один чек.
    sale_id        uuid references sales,
    -- Когда владельцу ушло «QR оплачен, чек не проведён».
    alerted_at     timestamptz,
    created_at     timestamptz not null default now()
);
create index bank_qr_waiting_idx on bank_qr (expires_at) where status = 'waiting';
create index bank_qr_unclaimed_idx on bank_qr (paid_at) where status = 'paid' and sale_id is null;
create index bank_qr_branch_idx on bank_qr (branch_id, created_at desc);
create unique index bank_qr_sale_idx on bank_qr (sale_id) where sale_id is not null;

-- Оплата картой по QR банка: ссылка на оплаченный QR. Один QR — одна оплата.
alter table sale_payments add column bank_qr_id uuid references bank_qr;
create unique index sale_payments_bank_qr_idx on sale_payments (bank_qr_id) where bank_qr_id is not null;

-- Операции из выписки банка по POS и QR. Повторная загрузка не дублирует (ключ операции).
create table bank_operations (
    id             uuid primary key,
    branch_id      uuid not null references branches,
    -- Безналичный счёт учёта, куда ложатся эти деньги (настройка «Optima Business»).
    account_id     uuid not null references cash_accounts,
    op_key         text not null,
    kind           text not null check (kind in ('card', 'qr')),
    processed_at   timestamptz not null,
    -- День операции по времени точки.
    op_day         date not null,
    amount_tyiyn   bigint not null,
    fee_tyiyn      bigint not null,
    transfer_tyiyn bigint not null,
    auth_code      text not null default '',
    rrn            text not null default '',
    terminal       text not null default '',
    requisite      text not null default '',
    -- Операция как её прислал банк: по ней видно, чем QR отличается от карты.
    raw            jsonb not null default '{}',
    user_id        uuid references users,
    loaded_at      timestamptz not null default now(),
    unique (branch_id, op_key)
);
create index bank_operations_day_idx on bank_operations (branch_id, account_id, op_day);
create trigger bank_operations_immutable before update or delete on bank_operations
    for each row execute function forbid_change();

-- Поправка комиссии банка по выписке: разница между комиссией банка за день и уже учтённой.
-- Каждая сверка дописывает только новую разницу; движение счёта — в cash_movements.
create table bank_fee_adjustments (
    id             uuid primary key,
    branch_id      uuid not null references branches,
    account_id     uuid not null references cash_accounts,
    op_day         date not null,
    -- Плюс — банк взял больше, чем учли по ставке; минус — меньше.
    amount_tyiyn   bigint not null,
    bank_fee_tyiyn bigint not null,
    our_fee_tyiyn  bigint not null,
    movement_id    uuid not null references cash_movements,
    user_id        uuid not null references users,
    created_at     timestamptz not null default now()
);
create index bank_fee_adjustments_day_idx on bank_fee_adjustments (branch_id, op_day);
create trigger bank_fee_adjustments_immutable before update or delete on bank_fee_adjustments
    for each row execute function forbid_change();
