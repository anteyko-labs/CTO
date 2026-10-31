-- Единица и объём канистры товара меняет владелец (задача 002, П4; задача 003, Н10; ADR-139).
-- Без остатка — сразу; с остатком — списание старым остатком и приход новым по той же
-- себестоимости одной операцией (движения `unit_change`). Прошлые документы остаются в прежних
-- единицах; запись ниже говорит, как их единицы переводятся в нынешние.
create table product_unit_changes (
    id                uuid primary key,
    product_id        uuid not null references products,
    from_unit         text not null check (from_unit in ('piece', 'ml')),
    from_container_ml bigint check (from_container_ml > 0),
    to_unit           text not null check (to_unit in ('piece', 'ml')),
    to_container_ml   bigint check (to_container_ml > 0),
    -- Остаток: новое = старое × qty_mul / qty_div, только целым числом.
    qty_mul           bigint not null check (qty_mul > 0),
    qty_div           bigint not null check (qty_div > 0),
    user_id           uuid not null references users,
    device_id         uuid not null,
    created_at        timestamptz not null default now(),
    check ((from_unit = 'ml') = (from_container_ml is not null)),
    check ((to_unit = 'ml') = (to_container_ml is not null))
);
create index product_unit_changes_product_idx on product_unit_changes (product_id, created_at);
create trigger product_unit_changes_immutable before update or delete on product_unit_changes
    for each row execute function forbid_change();
