-- Администратор в боте (SPEC-40): ожидание ввода в чате администратора — поиск остатка товара
-- и книжки по номеру (`admin_find`) и «💸 Расход» по шагам (`admin_expense`). Список в проверке
-- дополняется тем, что в нём уже есть (как 0078): значения других обновлений не теряются.
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
    foreach k in array array['admin_find', 'admin_expense'] loop
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
