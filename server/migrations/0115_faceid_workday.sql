-- Задача 039 A, ADR-200: рабочий день по кнопке в боте и лицу на терминале Dahua.

-- Терминал филиала: ключ в адресе приёма событий и окно подтверждения, мин.
create table faceid_devices (
    branch_id  uuid primary key references branches(id),
    key        text not null unique check (length(key) >= 16),
    window_min integer not null default 3 check (window_min between 1 and 30),
    updated_at timestamptz not null default now()
);

-- Номер сотрудника в терминале (UserID в Dahua); уникален в филиале.
alter table employees add column faceid_user_id text
    check (faceid_user_id is null or length(faceid_user_id) between 1 and 40);
create unique index employees_faceid_user_uq on employees (branch_id, faceid_user_id)
    where faceid_user_id is not null;

-- Журнал терминала: каждое принятое событие (только добавление), повтор — по dedup.
create table faceid_events (
    id          uuid primary key,
    branch_id   uuid references branches(id),
    dedup       text not null unique,
    user_id     text,
    employee_id uuid references employees(id),
    outcome     text not null check (outcome in
                ('applied', 'no_request', 'unknown_user', 'bad_key', 'no_user', 'failed_face')),
    payload     jsonb not null default '{}'::jsonb,
    created_at  timestamptz not null default now()
);
create index faceid_events_branch_idx on faceid_events (branch_id, created_at desc);

create or replace function faceid_events_append_only() returns trigger language plpgsql as $$
begin
    raise exception 'faceid_events: только добавление';
end
$$;
create trigger faceid_events_no_change before update or delete on faceid_events
    for each row execute function faceid_events_append_only();

-- Заявка «начать / закончить день» из бота: ждёт лица в окне.
create table workday_requests (
    id          uuid primary key,
    employee_id uuid not null references employees(id),
    branch_id   uuid not null references branches(id),
    kind        text not null check (kind in ('start', 'stop')),
    chat_id     bigint,
    status      text not null default 'pending'
                check (status in ('pending', 'done', 'expired', 'cancelled')),
    created_at  timestamptz not null default now(),
    expires_at  timestamptz not null,
    done_at     timestamptz,
    notified_at timestamptz
);
create index workday_requests_pending_idx on workday_requests (employee_id) where status = 'pending';
create index workday_requests_notify_idx on workday_requests (status) where notified_at is null;

-- Рабочие сессии: начало и конец дня; закрытый владельцем — с причиной.
create table work_sessions (
    id           uuid primary key,
    employee_id  uuid not null references employees(id),
    branch_id    uuid not null references branches(id),
    started_at   timestamptz not null,
    ended_at     timestamptz,
    closed_by    uuid references users(id),
    close_reason text,
    check (ended_at is null or ended_at >= started_at)
);
create unique index work_sessions_open_uq on work_sessions (employee_id) where ended_at is null;
create index work_sessions_branch_idx on work_sessions (branch_id, started_at);

-- Право «Табель» (timesheet), не по умолчанию; список прав в проверке дополняется, как в 0092.
do $$
declare
    def  text;
    keys text[];
begin
    select pg_get_constraintdef(oid) into def from pg_constraint
    where conrelid = 'public.user_permissions'::regclass
      and conname = 'user_permissions_permission_check';
    select coalesce(array_agg(t.m[1]), '{}') into keys
    from regexp_matches(coalesce(def, ''), '''([a-z_]+)''', 'g') as t(m);
    if not ('timesheet' = any (keys)) then
        keys := keys || 'timesheet'::text;
    end if;
    alter table public.user_permissions drop constraint if exists user_permissions_permission_check;
    execute format(
        'alter table public.user_permissions add constraint user_permissions_permission_check check (permission in (%s))',
        (select string_agg(quote_literal(k), ', ') from unnest(keys) as k));
end
$$;

-- Событие «Рабочий день» (staff.workday) — в ленте бота владельца по умолчанию выключено.
update settings
set value = jsonb_set(value, '{off}', coalesce(value -> 'off', '[]'::jsonb) || '["staff.workday"]'::jsonb)
where starts_with(key, 'bot_feed:')
  and jsonb_typeof(value) = 'object'
  and not coalesce(value -> 'off', '[]'::jsonb) ? 'staff.workday';
