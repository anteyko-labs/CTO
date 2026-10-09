-- Ревизия склада (SPEC-15): черновик пересчёта, проведение выравнивает остатки движениями.
create table revisions (
    id          uuid primary key,
    branch_id   uuid not null references branches,
    number      bigint not null,
    category_id uuid references categories,
    status      text not null default 'draft' check (status in ('draft', 'posted', 'cancelled')),
    comment     text not null default '',
    user_id     uuid not null references users,
    device_id   uuid not null,
    created_at  timestamptz not null default now(),
    posted_by   uuid references users,
    posted_at   timestamptz,
    unique (branch_id, number)
);

-- Пересчитанное в черновике правится: это ещё не учёт.
create table revision_lines (
    revision_id uuid not null references revisions,
    product_id  uuid not null references products,
    counted_qty bigint not null check (counted_qty >= 0),
    user_id     uuid not null references users,
    updated_at  timestamptz not null default now(),
    primary key (revision_id, product_id)
);

-- Итог проведённой ревизии неизменен (инвариант 4).
create table revision_results (
    revision_id       uuid not null references revisions,
    product_id        uuid not null references products,
    expected_qty      bigint not null,
    counted_qty       bigint not null,
    qty_delta         bigint not null,
    value_delta_tyiyn bigint not null,
    primary key (revision_id, product_id)
);
create trigger revision_results_immutable before update or delete on revision_results
    for each row execute function forbid_change();
