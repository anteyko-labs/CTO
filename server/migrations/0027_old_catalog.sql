-- Список товаров из старой программы (SPEC-20, ADR-056): только подсказка при заведении,
-- товаров учёта из него не создаётся, пока человек не подтвердит позицию.
create table old_catalog (
    id          uuid primary key,
    branch_id   uuid not null references branches,
    name        text not null,
    barcode     text not null default '',
    article     text not null default '',
    extra_code  text not null default '',
    cost_tyiyn  bigint not null default 0,
    price_tyiyn bigint not null default 0,
    unit        text not null default '',
    supplier    text not null default '',
    category    text not null default '',
    imported_at timestamptz not null default now()
);
create index old_catalog_barcode_idx on old_catalog (branch_id, barcode);
create index old_catalog_name_trgm_idx on old_catalog using gin (name gin_trgm_ops);
