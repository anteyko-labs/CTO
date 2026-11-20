-- Хвосты задач 037 и 039 (ADR-199, ADR-202).
-- Водитель фирмы из кабинета ждёт подтверждения, как машина (задача 037 п. 3).
alter table party_contacts add column status text not null default 'active'
    check (status in ('active', 'pending', 'rejected'));

create table contact_requests (
    id          uuid primary key,
    branch_id   uuid not null references branches,
    contact_id  uuid not null references party_contacts,
    party_id    uuid not null references parties,
    source      text not null check (source in ('cabinet', 'bot')),
    author      text not null default '',
    op_id       uuid unique,
    status      text not null default 'pending' check (status in ('pending', 'approved', 'rejected')),
    reason      text not null default '',
    decided_by  text not null default '',
    decided_at  timestamptz,
    created_at  timestamptz not null default now()
);
create index contact_requests_party_idx on contact_requests (party_id, created_at desc);

create function contact_requests_guard() returns trigger language plpgsql as $$
begin
    if tg_op = 'DELETE' then
        raise exception 'contact_requests: запись не удаляется';
    end if;
    if old.status <> 'pending' and row(new.*) is distinct from row(old.*) then
        raise exception 'contact_requests: решение уже принято';
    end if;
    return new;
end $$;
create trigger contact_requests_guard before update or delete on contact_requests
    for each row execute function contact_requests_guard();

-- Запрос фирме: машина чека (сообщение владельцу личной машины о решении), чек без сети,
-- когда владельцу личной машины сообщено.
alter table sale_approvals add column vehicle_id uuid references party_vehicles;
alter table sale_approvals add column offline boolean not null default false;
alter table sale_approvals add column owner_notified_at timestamptz;
create index sale_approvals_owner_notify_idx on sale_approvals (decided_at)
    where vehicle_id is not null and owner_notified_at is null and status in ('approved', 'rejected');

-- Бот ждёт причину отказа фирмы (`ap_reason`); список дополняется, как 0112.
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
    foreach k in array array['ap_reason'] loop
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
