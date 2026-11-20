-- Гаражи юрлица и подтверждение машин (задача 037 п. 1–3, ADR-199).
-- Машина из кабинета фирмы или личная машина работника «Оплачивает фирма» из бота ждёт
-- подтверждения: пока она не `active`, в долг фирмы её не провести.
alter table party_vehicles add column status text not null default 'active'
    check (status in ('active', 'pending', 'rejected'));
-- Личная машина работника, обслуживание которой оплачивает фирма: долг — на фирму.
alter table party_vehicles add column paid_by_party_id uuid references parties;
create index party_vehicles_paid_by_idx on party_vehicles (paid_by_party_id)
    where paid_by_party_id is not null;

-- Запросы на подтверждение: кто и откуда добавил, решение и причина. Строка не удаляется;
-- после решения меняется только отметка «автору сообщено».
create table vehicle_requests (
    id          uuid primary key,
    branch_id   uuid not null references branches,
    vehicle_id  uuid not null references party_vehicles,
    -- Фирма, которая подтверждает (и платит).
    party_id    uuid not null references parties,
    source      text not null check (source in ('cabinet', 'bot')),
    author      text not null default '',
    -- Чат автора в боте: ему — сообщение о решении.
    chat_id     bigint,
    op_id       uuid unique,
    status      text not null default 'pending' check (status in ('pending', 'approved', 'rejected')),
    reason      text not null default '',
    decided_by  text not null default '',
    decided_at  timestamptz,
    notified_at timestamptz,
    created_at  timestamptz not null default now()
);
create index vehicle_requests_party_idx on vehicle_requests (party_id, created_at desc);
create index vehicle_requests_notify_idx on vehicle_requests (decided_at)
    where chat_id is not null and notified_at is null and status <> 'pending';

create function vehicle_requests_guard() returns trigger language plpgsql as $$
begin
    if tg_op = 'DELETE' then
        raise exception 'vehicle_requests: запись не удаляется';
    end if;
    if old.status <> 'pending' and (new.status, new.reason, new.decided_by, new.decided_at,
            new.vehicle_id, new.party_id, new.author)
        is distinct from (old.status, old.reason, old.decided_by, old.decided_at,
            old.vehicle_id, old.party_id, old.author) then
        raise exception 'vehicle_requests: решение уже принято';
    end if;
    return new;
end $$;
create trigger vehicle_requests_guard before update or delete on vehicle_requests
    for each row execute function vehicle_requests_guard();

-- Бот ждёт кнопку «Оплачивает фирма» / «Плачу сам» (`payer`); список дополняется, как 0091.
do $$
declare
    def  text;
    keys text[];
    k    text;
begin
    select pg_get_constraintdef(oid) into def from pg_constraint
    where conrelid = 'public.bot_input'::regclass and conname = 'bot_input_kind_check';
    select coalesce(array_agg(m.v[1]), '{}') into keys
    from regexp_matches(coalesce(def, ''), '''([a-z_]+)''', 'g') as m(v);
    foreach k in array array['payer'] loop
        if not (k = any (keys)) then
            keys := keys || k;
        end if;
    end loop;
    alter table public.bot_input drop constraint if exists bot_input_kind_check;
    execute format(
        'alter table public.bot_input add constraint bot_input_kind_check check (kind in (%s))',
        (select string_agg(quote_literal(x), ', ') from unnest(keys) as x));
end
$$;
