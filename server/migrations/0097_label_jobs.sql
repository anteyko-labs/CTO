-- Задача 006, ADR-177: очередь печати этикеток на складском принтере («Станция этикеток»).
-- Телефон ставит задание, страница станции на ПК склада забирает его и печатает.
create table label_jobs (
    id uuid primary key,
    branch_id uuid not null references branches(id),
    op_id uuid not null unique,
    -- [{product_id, copies}]; строки этикетки берутся с сервера при печати.
    items jsonb not null,
    copies integer not null check (copies > 0 and copies <= 500),
    status text not null default 'queued' check (status in ('queued', 'printing', 'done', 'failed')),
    error text,
    created_by uuid not null references users(id),
    created_at timestamptz not null default now(),
    taken_at timestamptz,
    done_at timestamptz,
    station_id uuid
);
create index label_jobs_queue on label_jobs (branch_id, status, created_at);

-- Станции печати: отметка «жив» с каждым запросом станции.
create table label_stations (
    id uuid primary key,
    branch_id uuid not null references branches(id),
    user_id uuid not null references users(id),
    last_seen timestamptz not null default now()
);
create index label_stations_branch on label_stations (branch_id, last_seen);
