-- Задача 005 Н3, ADR-168: право «Привязывать коды на кассе» (till_barcode), группа «Касса».
-- Только добавить код к товару из окна «Код … не найден» на кассе, без правки карточки. По
-- умолчанию выдаётся: существующим администраторам — сразу (как acceptance в 0074). Список прав
-- в проверке дополняется тем, что в ней уже есть: права других обновлений не теряются при любом
-- их порядке (как в 0074 и 0080).
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
    if not ('till_barcode' = any (keys)) then
        keys := keys || 'till_barcode'::text;
    end if;
    alter table public.user_permissions drop constraint if exists user_permissions_permission_check;
    execute format(
        'alter table public.user_permissions add constraint user_permissions_permission_check check (permission in (%s))',
        (select string_agg(quote_literal(k), ', ') from unnest(keys) as k));
end
$$;

insert into user_permissions (user_id, branch_id, permission, granted_by)
select u.id, u.branch_id, 'till_barcode', null
from users u
where u.role = 'admin'
on conflict do nothing;

-- Событие «К товару привязан код» (product.barcode) — в ленте бота владельца по умолчанию
-- выключено (I21): уже сохранённым настройкам ленты оно добавляется в «выключенные».
update settings
set value = jsonb_set(value, '{off}', coalesce(value -> 'off', '[]'::jsonb) || '["product.barcode"]'::jsonb)
where starts_with(key, 'bot_feed:')
  and jsonb_typeof(value) = 'object'
  and not coalesce(value -> 'off', '[]'::jsonb) ? 'product.barcode';
