-- Задача 003 Н1, ADR-{B5.1}: право администратора «Наличные ↔ QR в открытой смене» (till_pay_fix).
-- По умолчанию не выдаётся: существующим администраторам ничего не добавляем, включает владелец.
-- Список прав в проверке дополняется тем, что в ней уже есть: права, добавленные другими
-- обновлениями, не теряются при любом их порядке (как в 0074).
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
    if not ('till_pay_fix' = any (keys)) then
        keys := keys || 'till_pay_fix'::text;
    end if;
    alter table public.user_permissions drop constraint if exists user_permissions_permission_check;
    execute format(
        'alter table public.user_permissions add constraint user_permissions_permission_check check (permission in (%s))',
        (select string_agg(quote_literal(k), ', ') from unnest(keys) as k));
end
$$;
