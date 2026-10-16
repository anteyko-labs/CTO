-- Минус по факту, комиссия банка по счёту, счёт по умолчанию для карты и для перевода
-- (решение заказчика от 17.10.2026, ADR-077).

-- 1. Деньги уходят в минус, когда так было на деле: учёт не блокирует проведение реальной
-- операции. Признак «отрицательный баланс разрешён» включается у всех счетов, и у кассы смены;
-- новый счёт — «разрешён». Владелец может выключить признак у любого счёта.
update cash_accounts set allow_negative = true;
alter table cash_accounts alter column allow_negative set default true;

-- 2. Счёт по умолчанию для карты и для перевода — две отметки вместо одного «основного банка».
-- Прежний основной безналичный счёт становится счётом по умолчанию для обоих способов.
alter table cash_accounts add column default_card boolean not null default false;
alter table cash_accounts add column default_transfer boolean not null default false;
update cash_accounts set default_card = true, default_transfer = true
where kind = 'bank' and is_default;
alter table cash_accounts add constraint cash_accounts_default_bank_check
    check ((not default_card and not default_transfer) or kind = 'bank');
create unique index cash_accounts_default_card on cash_accounts (branch_id) where default_card;
create unique index cash_accounts_default_transfer on cash_accounts (branch_id) where default_transfer;
-- `is_default` остаётся только у кассы смены.
drop index cash_accounts_default_bank;
update cash_accounts set is_default = false where kind = 'bank' and is_default;

-- 3. Ставка комиссии банка — у безналичного счёта, с историей, как прежние ставки по способу
-- оплаты (SPEC-08, ADR-065): новая ставка закрывает прежнюю, прошлые дни сохраняют свою.
create table cash_account_fees (
    id          uuid primary key,
    branch_id   uuid not null references branches,
    account_id  uuid not null references cash_accounts,
    rate_bp     integer not null check (rate_bp between 0 and 10000),
    active_from date not null,
    active_to   date,
    user_id     uuid references users,
    created_at  timestamptz not null default now(),
    check (active_to is null or active_to >= active_from)
);
-- Одна открытая ставка у счёта.
create unique index cash_account_fees_open_idx on cash_account_fees (account_id)
    where active_to is null;
create index cash_account_fees_lookup_idx on cash_account_fees (account_id, active_from);

-- Ставка счёта, действовавшая в день `on`; нет ставки — 0. Одна функция для чеков, погашений
-- и показа ставки на сегодня.
create function account_fee_on(account uuid, on_day date) returns integer
language sql stable as $$
    select coalesce((select f.rate_bp from cash_account_fees f
                     where f.account_id = account
                       and f.active_from <= on_day
                       and (f.active_to is null or f.active_to > on_day)
                     order by f.active_from desc, f.created_at desc limit 1), 0)
$$;

-- Прежняя общая ставка (вся история ставки за карту) переходит на прежний основной счёт «Счёт»:
-- карта с терминала шла на него. Остальные безналичные счета — 0 %, ставку им ставит владелец.
insert into cash_account_fees (id, branch_id, account_id, rate_bp, active_from, active_to,
                               user_id, created_at)
select gen_random_uuid(), f.branch_id, a.id, f.rate_bp, f.active_from, f.active_to,
       f.user_id, f.created_at
from payment_method_fees f
join cash_accounts a on a.branch_id = f.branch_id and a.kind = 'bank' and a.default_card
where f.method = 'card';
