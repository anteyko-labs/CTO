-- Задача 037 п.5, ADR-203: график выходных сотрудников; минимум работающих — настройка филиала
-- `settings(key='staff').value.min_on_duty` (нет — 0, без ограничения).
create table employee_days_off (
    employee_id uuid not null references employees(id),
    day         date not null,
    branch_id   uuid not null references branches(id),
    created_by  uuid references users(id),
    created_at  timestamptz not null default now(),
    primary key (employee_id, day)
);
create index employee_days_off_branch_day_idx on employee_days_off (branch_id, day);

-- Право «График выходных» (staff_schedule), не по умолчанию; список прав в проверке дополняется, как в 0092.
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
    if not ('staff_schedule' = any (keys)) then
        keys := keys || 'staff_schedule'::text;
    end if;
    alter table public.user_permissions drop constraint if exists user_permissions_permission_check;
    execute format(
        'alter table public.user_permissions add constraint user_permissions_permission_check check (permission in (%s))',
        (select string_agg(quote_literal(k), ', ') from unnest(keys) as k));
end
$$;
