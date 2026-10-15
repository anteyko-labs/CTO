-- История ставок комиссии банка и отмена приёма аккумуляторов (ADR-065).

-- Ставка комиссии действует с даты: новая закрывает прежнюю, к чеку применяется ставка,
-- действовавшая в его учётный день (SPEC-08). Прежние ставки считаются действующими «всегда».
alter table payment_method_fees add column id uuid;
update payment_method_fees set id = gen_random_uuid();
alter table payment_method_fees alter column id set not null;
alter table payment_method_fees drop constraint payment_method_fees_pkey;
alter table payment_method_fees add primary key (id);
alter table payment_method_fees add column active_from date not null default date '2000-01-01';
alter table payment_method_fees alter column active_from drop default;
alter table payment_method_fees add column active_to date;
alter table payment_method_fees add column user_id uuid references users;
alter table payment_method_fees add column created_at timestamptz not null default now();
alter table payment_method_fees add constraint payment_method_fees_period_check
    check (active_to is null or active_to >= active_from);
-- Одна открытая ставка на способ оплаты в филиале.
create unique index payment_method_fees_open_idx on payment_method_fees (branch_id, method)
    where active_to is null;
create index payment_method_fees_lookup_idx on payment_method_fees (branch_id, method, active_from);

-- Отмена приёма аккумулятора: обратные движения склада и кассы, документ приёма неизменен.
create table battery_intake_reversals (
    id         uuid primary key,
    branch_id  uuid not null references branches,
    intake_id  uuid not null unique references battery_intakes,
    comment    text not null default '',
    user_id    uuid not null references users,
    device_id  uuid not null,
    created_at timestamptz not null default now()
);

create trigger battery_intake_reversals_immutable before update or delete on battery_intake_reversals
    for each row execute function forbid_change();
