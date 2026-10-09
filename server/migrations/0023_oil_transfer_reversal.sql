-- Отмена перелива (SPEC-14, ADR-052): обратные движения той же стоимостью, документ неизменен.
create table oil_transfer_reversals (
    id          uuid primary key,
    branch_id   uuid not null references branches,
    transfer_id uuid not null unique references oil_transfers,
    comment     text not null,
    user_id     uuid not null references users,
    device_id   uuid not null,
    created_at  timestamptz not null default now()
);

create trigger oil_transfer_reversals_immutable before update or delete on oil_transfer_reversals
    for each row execute function forbid_change();
