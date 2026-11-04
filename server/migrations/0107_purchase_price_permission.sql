-- Задача 031: право «Менять цену закупки товара» (ADR-192). По умолчанию выключено, существующим
-- администраторам не выдаётся: раньше это было только у владельца.
-- Список прав в проверке дополняется тем, что в ней уже есть (как 0074, 0081).
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
    if not ('purchase_price' = any (keys)) then
        keys := keys || 'purchase_price'::text;
    end if;
    alter table public.user_permissions drop constraint if exists user_permissions_permission_check;
    execute format(
        'alter table public.user_permissions add constraint user_permissions_permission_check check (permission in (%s))',
        (select string_agg(quote_literal(x), ', ') from unnest(keys) as x));
end
$$;
