-- Задача 038: свой товар на нужды магазина (ADR-198). Товар уходит со склада движением
-- `own_use` по средней себестоимости, как брак; деньги кассы не трогаются. Себестоимость —
-- отдельная строка прибыли «Товар на нужды магазина» по дню записи. Сторно — новая строка со
-- ссылкой и противоположным знаком.
create table own_use_writeoffs (
    id          uuid primary key,
    branch_id   uuid not null references branches,
    number      bigint not null,
    product_id  uuid not null references products,
    -- > 0 у списания, < 0 у сторно (в единицах хранения товара, I24).
    qty         bigint not null check (qty <> 0),
    cost_tyiyn  bigint not null,
    comment     text not null default '' check (char_length(comment) <= 500),
    reversal_of uuid references own_use_writeoffs,
    user_id     uuid not null references users,
    device_id   uuid not null,
    created_at  timestamptz not null default now(),
    unique (branch_id, number)
);
create unique index own_use_reversal_once on own_use_writeoffs (reversal_of) where reversal_of is not null;
create index own_use_branch_idx on own_use_writeoffs (branch_id, created_at desc);
create index own_use_product_idx on own_use_writeoffs (product_id);
create trigger own_use_writeoffs_immutable before update or delete on own_use_writeoffs
    for each row execute function forbid_change();
