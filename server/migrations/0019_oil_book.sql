-- Масляная книжка (SPEC-16): замены по машинам клиента и интервал до следующей.
alter table party_vehicles add column interval_km integer not null default 8000 check (interval_km > 0);
alter table party_vehicles add column interval_months integer not null default 6 check (interval_months > 0);

create table oil_changes (
    id          uuid primary key,
    branch_id   uuid not null references branches,
    vehicle_id  uuid not null references party_vehicles,
    party_id    uuid not null references parties,
    sale_id     uuid references sales,
    change_date date not null,
    mileage_km  integer check (mileage_km >= 0),
    oil_text    text not null default '',
    filter_text text not null default '',
    comment     text not null default '',
    user_id     uuid not null references users,
    created_at  timestamptz not null default now(),
    updated_at  timestamptz not null default now()
);
create index oil_changes_vehicle_idx on oil_changes (vehicle_id, change_date desc);
create unique index oil_changes_sale_once on oil_changes (sale_id) where sale_id is not null;
