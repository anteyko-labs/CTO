-- Задача 052 (ADR-217): WhatsApp по QR-коду — канал `whatsapp_qr` в журнале сообщений клиентам
-- и переписка номера магазина (входящие, ответы бота, ответы человека с телефона).
do $$
declare
    def  text;
    keys text[];
begin
    select pg_get_constraintdef(oid) into def from pg_constraint
    where conrelid = 'public.client_messages'::regclass and conname = 'client_messages_channel_check';
    select coalesce(array_agg(m.v[1]), '{}') into keys
    from regexp_matches(coalesce(def, ''), '''([a-z_]+)''', 'g') as m(v);
    if not ('whatsapp_qr' = any (keys)) then
        keys := keys || 'whatsapp_qr'::text;
    end if;
    execute 'alter table public.client_messages drop constraint if exists client_messages_channel_check';
    execute format(
        'alter table public.client_messages add constraint client_messages_channel_check check (channel in (%s))',
        (select string_agg(quote_literal(x), ', ') from unnest(keys) as x));
end
$$;

create table whatsapp_inbox (
    id         uuid primary key,
    phone      text not null,
    party_id   uuid references parties,
    direction  text not null check (direction in ('in', 'out', 'human')),
    text       text not null default '',
    created_at timestamptz not null default now()
);
create index whatsapp_inbox_phone_idx on whatsapp_inbox (phone, created_at desc);
create index whatsapp_inbox_created_idx on whatsapp_inbox (created_at desc);
