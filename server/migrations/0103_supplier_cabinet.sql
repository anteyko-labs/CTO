-- Кабинет поставщика (задача 028, ADR-188).

-- 1. Файл накладной может приложить сам поставщик из кабинета: тогда пользователя точки нет,
--    вместо него — поставщик. Ровно одно из двух.
alter table receipt_files
    add column source text not null default 'staff' check (source in ('staff', 'supplier')),
    add column party_id uuid references parties,
    alter column user_id drop not null,
    alter column device_id drop not null,
    add constraint receipt_files_author check (
        (source = 'staff' and user_id is not null and device_id is not null and party_id is null)
        or (source = 'supplier' and user_id is null and party_id is not null));

-- 2. Сообщение поставщику о принятой и отменённой накладной — вид `invoice` (дополнение
--    проверки тем, что в ней уже есть, как 0078).
do $$
declare
    def  text;
    keys text[];
begin
    select pg_get_constraintdef(oid) into def from pg_constraint
    where conrelid = 'public.client_messages'::regclass and conname = 'client_messages_kind_check';
    select coalesce(array_agg(m.v[1]), '{}') into keys
    from regexp_matches(coalesce(def, ''), '''([a-z_]+)''', 'g') as m(v);
    if not ('invoice' = any (keys)) then
        keys := keys || 'invoice'::text;
    end if;
    alter table public.client_messages drop constraint if exists client_messages_kind_check;
    execute format('alter table public.client_messages add constraint client_messages_kind_check check (kind in (%s))',
        (select string_agg(quote_literal(x), ', ') from unnest(keys) as x));
end
$$;
