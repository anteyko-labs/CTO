-- Ядро этапа 1: доступ, журнал, каталог, склад, приход, касса.

create extension if not exists pg_trgm;

-- Проведённые записи не меняются и не удаляются (ADR-014).
create function forbid_change() returns trigger language plpgsql as $$
begin
    raise exception 'таблица % неизменяема', tg_table_name using errcode = 'P0001';
end
$$;

create table branches (
    id         uuid primary key,
    name       text not null,
    created_at timestamptz not null default now()
);

create table users (
    id            uuid primary key,
    branch_id     uuid not null references branches,
    login         text not null,
    password_hash text not null,
    role          text not null check (role in ('owner', 'admin')),
    full_name     text not null,
    active        boolean not null default true,
    created_at    timestamptz not null default now()
);
create unique index users_login_uq on users (lower(login));

create table sessions (
    token_hash bytea primary key,
    user_id    uuid not null references users,
    device_id  uuid,
    created_at timestamptz not null default now(),
    expires_at timestamptz not null
);
create index sessions_user_idx on sessions (user_id);

create table operations (
    op_id      uuid primary key,
    user_id    uuid not null references users,
    kind       text not null,
    response   jsonb not null,
    created_at timestamptz not null default now()
);

create table audit_log (
    id         uuid primary key,
    branch_id  uuid not null references branches,
    user_id    uuid references users,
    device_id  uuid,
    action     text not null,
    entity     text not null,
    entity_id  uuid,
    data       jsonb not null default '{}',
    created_at timestamptz not null default now()
);
create index audit_log_branch_time_idx on audit_log (branch_id, created_at);

create table counters (
    branch_id uuid not null references branches,
    name      text not null,
    value     bigint not null,
    primary key (branch_id, name)
);

create table settings (
    branch_id uuid not null references branches,
    key       text not null,
    value     jsonb not null,
    primary key (branch_id, key)
);

-- Справочники

create table employees (
    id         uuid primary key,
    branch_id  uuid not null references branches,
    full_name  text not null,
    is_cashier boolean not null default false,
    is_master  boolean not null default false,
    active     boolean not null default true,
    created_at timestamptz not null default now()
);

create table services (
    id               uuid primary key,
    branch_id        uuid not null references branches,
    name             text not null,
    price_tyiyn      bigint not null check (price_tyiyn >= 0),
    master_fee_tyiyn bigint not null default 3000 check (master_fee_tyiyn >= 0),
    active           boolean not null default true,
    created_at       timestamptz not null default now()
);

create table suppliers (
    id         uuid primary key,
    name       text not null,
    phone      text not null default '',
    comment    text not null default '',
    active     boolean not null default true,
    created_at timestamptz not null default now()
);

-- Каталог

create table categories (
    id         uuid primary key,
    name       text not null,
    kind       text not null check (kind in ('oil', 'filter', 'battery', 'other')),
    attributes jsonb not null default '[]',
    created_at timestamptz not null default now()
);

create table products (
    id           uuid primary key,
    category_id  uuid not null references categories,
    name         text not null,
    brand        text not null default '',
    article      text not null default '',
    unit         text not null check (unit in ('piece', 'ml')),
    container_ml bigint check (container_ml > 0),
    attrs        jsonb not null default '{}',
    archived     boolean not null default false,
    created_at   timestamptz not null default now(),
    check ((unit = 'ml') = (container_ml is not null))
);
create index products_name_trgm_idx on products using gin (name gin_trgm_ops);
create index products_category_idx on products (category_id);

create table product_barcodes (
    code       text primary key,
    product_id uuid not null references products,
    internal   boolean not null default false,
    created_at timestamptz not null default now()
);
create index product_barcodes_product_idx on product_barcodes (product_id);

-- Номера собственных штрихкодов, общие для всех филиалов.
create sequence internal_barcode_seq;

-- Цены и кэш остатка товара в филиале. Кэш меняется только вместе с записью в stock_movements.
create table branch_products (
    branch_id             uuid not null references branches,
    product_id            uuid not null references products,
    sale_price_tyiyn      bigint not null default 0 check (sale_price_tyiyn >= 0),
    pour_price_per_l_tyiyn bigint check (pour_price_per_l_tyiyn >= 0),
    min_stock             bigint not null default 0 check (min_stock >= 0),
    stock_qty             bigint not null default 0,
    stock_value_tyiyn     bigint not null default 0,
    last_cost_qty         bigint not null default 0,
    last_cost_tyiyn       bigint not null default 0,
    needs_review          boolean not null default false,
    primary key (branch_id, product_id)
);

create table stock_movements (
    id                uuid primary key,
    branch_id         uuid not null references branches,
    product_id        uuid not null references products,
    qty_delta         bigint not null,
    value_delta_tyiyn bigint not null,
    doc_type          text not null,
    doc_id            uuid not null,
    created_at        timestamptz not null default now()
);
create index stock_movements_product_idx on stock_movements (branch_id, product_id);

-- Приход

create table receipts (
    id           uuid primary key,
    branch_id    uuid not null references branches,
    number       bigint not null,
    supplier_id  uuid references suppliers,
    supplier_doc text not null default '',
    comment      text not null default '',
    total_tyiyn  bigint not null,
    reversal_of  uuid unique references receipts,
    user_id      uuid not null references users,
    device_id    uuid not null,
    created_at   timestamptz not null default now(),
    unique (branch_id, number)
);

create table receipt_lines (
    id          uuid primary key,
    receipt_id  uuid not null references receipts,
    line_no     integer not null,
    product_id  uuid not null references products,
    qty         bigint not null,
    cost_tyiyn  bigint not null,
    unique (receipt_id, line_no)
);

-- Касса

create table sales (
    id          uuid primary key,
    branch_id   uuid not null references branches,
    number      bigint not null,
    kind        text not null check (kind in ('sale', 'return')),
    sale_type   text not null check (sale_type in ('takeaway', 'service')),
    cashier_id  uuid not null references employees,
    master_id   uuid references employees,
    total_tyiyn bigint not null,
    comment     text not null default '',
    reversal_of uuid references sales,
    user_id     uuid not null references users,
    device_id   uuid not null,
    client_time timestamptz,
    created_at  timestamptz not null default now(),
    unique (branch_id, number),
    check ((kind = 'return') = (reversal_of is not null))
);
create index sales_branch_time_idx on sales (branch_id, created_at);
create index sales_reversal_idx on sales (reversal_of);

create table sale_lines (
    id               uuid primary key,
    sale_id          uuid not null references sales,
    line_no          integer not null,
    kind             text not null check (kind in ('piece', 'container', 'pour', 'service')),
    product_id       uuid references products,
    service_id       uuid references services,
    qty              bigint not null,
    units            bigint not null,
    unit_price_tyiyn bigint not null,
    list_price_tyiyn bigint not null,
    amount_tyiyn     bigint not null,
    cost_tyiyn       bigint not null,
    master_fee_tyiyn bigint not null default 0,
    unique (sale_id, line_no),
    check ((kind = 'service') = (service_id is not null)),
    check ((kind = 'service') = (product_id is null))
);

create table sale_payments (
    id           uuid primary key,
    sale_id      uuid not null references sales,
    method       text not null check (method in ('cash', 'card', 'transfer')),
    amount_tyiyn bigint not null
);
create index sale_payments_sale_idx on sale_payments (sale_id);

create trigger audit_log_immutable before update or delete on audit_log
    for each row execute function forbid_change();
create trigger operations_immutable before update or delete on operations
    for each row execute function forbid_change();
create trigger stock_movements_immutable before update or delete on stock_movements
    for each row execute function forbid_change();
create trigger receipts_immutable before update or delete on receipts
    for each row execute function forbid_change();
create trigger receipt_lines_immutable before update or delete on receipt_lines
    for each row execute function forbid_change();
create trigger sales_immutable before update or delete on sales
    for each row execute function forbid_change();
create trigger sale_lines_immutable before update or delete on sale_lines
    for each row execute function forbid_change();
create trigger sale_payments_immutable before update or delete on sale_payments
    for each row execute function forbid_change();
