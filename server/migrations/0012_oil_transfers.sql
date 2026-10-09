-- Перелив масла (SPEC-14): остаток и его стоимость переходят из одного масла в другое,
-- себестоимость получателя становится средневзвешенной. Операция владельца.
create table oil_transfers (
    id              uuid primary key,
    branch_id       uuid not null references branches,
    number          bigint not null,
    from_product_id uuid not null references products,
    to_product_id   uuid not null references products,
    qty_ml          bigint not null check (qty_ml > 0),
    value_tyiyn     bigint not null check (value_tyiyn >= 0),
    comment         text not null default '',
    user_id         uuid not null references users,
    device_id       uuid not null,
    created_at      timestamptz not null default now(),
    check (from_product_id <> to_product_id),
    unique (branch_id, number)
);
create index oil_transfers_branch_idx on oil_transfers (branch_id, created_at desc);

create trigger oil_transfers_immutable before update or delete on oil_transfers
    for each row execute function forbid_change();
