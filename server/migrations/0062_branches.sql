-- Несколько филиалов на одном общем сервере (SPEC-33, ADR-108).
-- Существующая база — один главный филиал: ни одна строка не переносится и не удаляется,
-- остатки и балансы остаются суммой своих движений.

-- 1. Карточка филиала: адрес, телефон, закрытие (архив). Главный — первый заведённый: в него
-- смотрит владелец по умолчанию, из него новый филиал копирует настройки.
alter table branches
    add column address  text not null default '',
    add column phone    text not null default '',
    add column archived boolean not null default false,
    add column is_main  boolean not null default false;
update branches set is_main = true
where id = (select id from branches order by created_at, id limit 1);
create unique index branches_one_main on branches (is_main) where is_main;
create unique index branches_name_uq on branches (lower(name));
alter table branches add constraint branches_name_check check (length(name) between 1 and 100);
alter table branches add constraint branches_main_open check (not (is_main and archived));

-- 2. Цена продажи: общая на все филиалы (`products.sale_price_tyiyn`) и своя у филиала
-- (`branch_products.own_price`). В строке филиала `sale_price_tyiyn` — действующая цена этого
-- филиала: общая, пока своей нет. Касса и отчёты читают её, как раньше.
alter table products add column sale_price_tyiyn bigint not null default 0
    check (sale_price_tyiyn >= 0);
update products p set sale_price_tyiyn = bp.sale_price_tyiyn
from branch_products bp join branches b on b.id = bp.branch_id and b.is_main
where bp.product_id = p.id;
alter table branch_products add column own_price boolean not null default false;
-- Цена филиала уже отличается от общей (у главного так не бывает) — она своя.
update branch_products bp set own_price = true
from products p
where p.id = bp.product_id and bp.sale_price_tyiyn <> p.sale_price_tyiyn;

-- Новая строка филиала без своей цены берёт общую: строки заводятся и лениво (первое движение).
create function branch_products_shared_price() returns trigger language plpgsql as $$
begin
    if not new.own_price then
        new.sale_price_tyiyn := coalesce(
            (select p.sale_price_tyiyn from products p where p.id = new.product_id),
            new.sale_price_tyiyn);
    end if;
    return new;
end $$;
create trigger branch_products_shared_price before insert on branch_products
    for each row execute function branch_products_shared_price();

-- 3. Долг контрагента по филиалам: `parties.balance_tyiyn` — общий итог, здесь — его части.
-- Ведётся триггером из записей `party_ledger` (у каждой записи свой филиал), поэтому сумма
-- частей всегда равна сумме записей. Расхождение старой базы (итог без записей) — в главный.
create table party_balances (
    party_id      uuid not null references parties,
    branch_id     uuid not null references branches,
    balance_tyiyn bigint not null default 0,
    primary key (party_id, branch_id)
);
create index party_balances_branch_idx on party_balances (branch_id) where balance_tyiyn <> 0;
insert into party_balances (party_id, branch_id, balance_tyiyn)
select party_id, branch_id, sum(amount_tyiyn) from party_ledger group by party_id, branch_id;
insert into party_balances (party_id, branch_id, balance_tyiyn)
select p.id, b.id, p.balance_tyiyn - coalesce((select sum(l.amount_tyiyn) from party_ledger l
                                                where l.party_id = p.id), 0)
from parties p cross join branches b
where b.is_main
  and p.balance_tyiyn <> coalesce((select sum(l.amount_tyiyn) from party_ledger l
                                   where l.party_id = p.id), 0)
on conflict (party_id, branch_id)
do update set balance_tyiyn = party_balances.balance_tyiyn + excluded.balance_tyiyn;

create function party_ledger_branch_balance() returns trigger language plpgsql as $$
begin
    insert into party_balances (party_id, branch_id, balance_tyiyn)
    values (new.party_id, new.branch_id, new.amount_tyiyn)
    on conflict (party_id, branch_id)
    do update set balance_tyiyn = party_balances.balance_tyiyn + excluded.balance_tyiyn;
    return new;
end $$;
create trigger party_ledger_branch_balance after insert on party_ledger
    for each row execute function party_ledger_branch_balance();

-- 4. «Расчёты между филиалами»: клиент погасил в филиале B долг, взятый в филиале A, — деньги
-- в кассе B, долг закрыт у A, значит B должен A. Запись не правится (сторно — встречной).
-- `branch_id` — филиал, принявший деньги (должник), `creditor_branch_id` — чей долг закрыт.
create table branch_settlements (
    id                 uuid primary key,
    branch_id          uuid not null references branches,
    creditor_branch_id uuid not null references branches,
    party_id           uuid not null references parties,
    amount_tyiyn       bigint not null check (amount_tyiyn <> 0),
    business_date      date not null,
    -- Часть погашения, записанная на филиал A (запись `party_ledger` с `branch_id` = A).
    ledger_id          uuid not null references party_ledger,
    reversal_of        uuid references branch_settlements,
    user_id            uuid not null references users,
    created_at         timestamptz not null default now(),
    check (branch_id <> creditor_branch_id)
);
create index branch_settlements_pair_idx on branch_settlements (branch_id, creditor_branch_id);
create index branch_settlements_ledger_idx on branch_settlements (ledger_id);
create unique index branch_settlements_reversal_once on branch_settlements (reversal_of)
    where reversal_of is not null;
create trigger branch_settlements_immutable before update or delete on branch_settlements
    for each row execute function forbid_change();

-- 5. Общие справочники: правило подарка — одно на товар для всех филиалов.
alter table gift_rules drop constraint gift_rules_branch_id_trigger_product_id_key;
create unique index gift_rules_trigger_uq on gift_rules (trigger_product_id);

-- Поиск машины по номеру и клиента — по всей базе, без филиала.
create index party_vehicles_plate_all_idx on party_vehicles (plate_norm);
create index parties_role_idx on parties (role, active);
