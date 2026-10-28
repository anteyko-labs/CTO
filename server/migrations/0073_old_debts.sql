-- Старый долг из тетради (SPEC-38, ADR-133). Таблиц не добавляется: старый долг — запись
-- `party_ledger` вида `debt` с документом `old_debt`, учётный день — дата долга из тетради.
--
-- Право администратора «Старые долги из тетради» (`old_debts`) — по умолчанию никому не выдаётся.
-- Список прав в проверке дополняется, а не переписывается заново: права, которые добавили
-- другие миграции того же выпуска, в проверке остаются.
do $$
declare
    allowed text[];
begin
    select array_agg(distinct m[1] order by m[1]) into allowed
    from pg_constraint c
    cross join lateral regexp_matches(pg_get_constraintdef(c.oid), '''([a-z_]+)''', 'g') as m
    where c.conrelid = 'public.user_permissions'::regclass
      and c.conname = 'user_permissions_permission_check';
    if allowed is null then
        raise exception 'нет проверки списка прав user_permissions_permission_check';
    end if;
    if not 'old_debts' = any (allowed) then
        allowed := allowed || 'old_debts'::text;
    end if;
    alter table public.user_permissions drop constraint user_permissions_permission_check;
    -- Тем же видом `permission in (...)`, что прежние миграции: список читается так же.
    execute format(
        'alter table public.user_permissions add constraint user_permissions_permission_check '
        'check (permission in (%s))',
        (select string_agg(quote_literal(p), ', ' order by p) from unnest(allowed) as p));
end $$;
