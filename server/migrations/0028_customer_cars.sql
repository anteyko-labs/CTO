-- Машины клиента в боте: номер в одном виде, пометки клиента, ожидание ввода (SPEC-22, ADR-061).

-- Номер в одном виде. Повторяет domain::plate::normalize: латинские строчные — в заглавные,
-- кириллица, похожая на латиницу, — в латиницу, пробелы и дефисы убираются.
-- upper() не берём: для не-ASCII он зависит от локали базы, а правило должно быть одно.
create function plate_norm(p text) returns text
    language sql immutable strict parallel safe
    return translate(
        p,
        'abcdefghijklmnopqrstuvwxyzАВЕКМНОРСТУХавекмнорстух' || E' \t\n\r -',
        'ABCDEFGHIJKLMNOPQRSTUVWXYZABEKMHOPCTYXABEKMHOPCTYX'
    );

alter table party_vehicles add column plate_norm text not null default '';
update party_vehicles set plate_norm = plate_norm(plate);
alter table party_vehicles alter column plate_norm drop default;
create index party_vehicles_plate_norm_idx on party_vehicles (party_id, plate_norm);
-- Поиск номера по всей точке: только чтобы предупредить владельца о совпадении.
create index party_vehicles_branch_plate_idx on party_vehicles (branch_id, plate_norm);

-- Кто завёл машину: точка или сам клиент в боте.
alter table party_vehicles add column added_by text not null default 'staff'
    check (added_by in ('staff', 'client'));

-- Пометки клиента к машине. Точка их не правит; клиент удаляет свою — отметкой времени.
create table vehicle_notes (
    id         uuid primary key,
    branch_id  uuid not null references branches,
    vehicle_id uuid not null references party_vehicles,
    party_id   uuid not null references parties,
    text       text not null check (length(text) between 1 and 500),
    created_at timestamptz not null default now(),
    deleted_at timestamptz
);
create index vehicle_notes_vehicle_idx on vehicle_notes (vehicle_id, created_at desc);

-- Бот ждёт ввода от клиента (номер, марка, пометка) — одно ожидание на чат, 10 минут.
create table bot_input (
    chat_id    bigint primary key,
    branch_id  uuid not null references branches,
    kind       text not null check (kind in ('plate', 'brand', 'note')),
    vehicle_id uuid references party_vehicles,
    data       jsonb not null default '{}',
    expires_at timestamptz not null
);
