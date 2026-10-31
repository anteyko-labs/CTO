-- Сообщения должникам и долги в боте (задача 002, волна 2: П7, П7б, П8) и склейка фото накладной
-- в боте (задача 003, Н6).

-- 1. Виды сообщений клиентам: «Получили оплату» (`payment`) и «раз в месяц — кто сколько должен»
--    (`monthly`); лента «Сообщения» в кабинете юрлица — канал `cabinet` (строка журнала, которая
--    никуда не отправляется, а сразу видна в кабинете). Ожидание ввода в боте владельца —
--    «➕ Занести долг» по шагам (`old_debt`). Списки в проверках дополняются тем, что в них уже есть
--    (как 0074 §3): значения, добавленные другими обновлениями, не теряются при любом их порядке.
do $$
declare
    r    record;
    def  text;
    keys text[];
    k    text;
begin
    for r in
        select * from (values
            ('client_messages', 'client_messages_kind_check', 'kind', array['payment', 'monthly']),
            ('client_messages', 'client_messages_channel_check', 'channel', array['cabinet']),
            ('bot_input', 'bot_input_kind_check', 'kind', array['old_debt'])
        ) as c(tbl, con, col, added)
    loop
        select pg_get_constraintdef(oid) into def from pg_constraint
        where conrelid = ('public.' || r.tbl)::regclass and conname = r.con;
        select coalesce(array_agg(m.v[1]), '{}') into keys
        from regexp_matches(coalesce(def, ''), '''([a-z_]+)''', 'g') as m(v);
        foreach k in array r.added loop
            if not (k = any (keys)) then
                keys := keys || k;
            end if;
        end loop;
        execute format('alter table public.%I drop constraint if exists %I', r.tbl, r.con);
        execute format(
            'alter table public.%I add constraint %I check (%I in (%s))',
            r.tbl, r.con, r.col,
            (select string_agg(quote_literal(x), ', ') from unnest(keys) as x));
    end loop;
end
$$;

-- «Получили оплату» — только о погашениях после установки: прежние не рассылаются.
insert into bot_state (key, value)
values ('client_messages_payments_since', to_jsonb(to_char(now() at time zone 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')))
on conflict (key) do nothing;

-- Новые погашения ищутся по времени записи.
create index if not exists party_ledger_repayment_idx on party_ledger (created_at)
    where kind = 'repayment';

-- 2. Склейка фото накладной в боте (Н6): фото, дописанное к открытому черновику, можно отделить
--    кнопкой «➕ Это другая накладная» — ссылка на первый черновик снимается, пока черновик
--    открыт. Остальное — как в 0075: ссылка ставится при записи и иначе не меняется.
create or replace function receipt_drafts_guard() returns trigger language plpgsql as $$
begin
    if tg_op = 'DELETE' then
        raise exception 'черновик прихода не удаляется' using errcode = 'P0001';
    end if;
    if (new.id, new.branch_id, new.source, new.op_id, new.chat_id, new.name, new.content_type,
        new.size_bytes, new.user_id, new.device_id, new.created_at)
       is distinct from
       (old.id, old.branch_id, old.source, old.op_id, old.chat_id, old.name, old.content_type,
        old.size_bytes, old.user_id, old.device_id, old.created_at) then
        raise exception 'черновик прихода не меняется' using errcode = 'P0001';
    end if;
    if new.parent_id is distinct from old.parent_id
       and not (old.parent_id is not null and new.parent_id is null and old.status = 'new') then
        raise exception 'черновик прихода не меняется' using errcode = 'P0001';
    end if;
    if (new.recognition, new.recognized, new.error, new.input_tokens, new.output_tokens)
       is distinct from
       (old.recognition, old.recognized, old.error, old.input_tokens, old.output_tokens)
       and old.recognition <> 'pending' then
        raise exception 'распознавание черновика уже записано' using errcode = 'P0001';
    end if;
    if old.status not in ('new', 'acceptance')
       and (new.status, new.receipt_id, new.file_id, new.data, new.closed_at, new.closed_by)
           is distinct from
           (old.status, old.receipt_id, old.file_id, old.data, old.closed_at, old.closed_by) then
        raise exception 'черновик прихода уже закрыт' using errcode = 'P0001';
    end if;
    if new.data is distinct from old.data and not (new.data is null and new.status = 'used') then
        raise exception 'фото черновика не меняется' using errcode = 'P0001';
    end if;
    return new;
end
$$;
