-- Сообщения клиентам: Телеграм и WhatsApp (SPEC-32, ADR-107).

-- 1. Скрытый код клиента для ссылки на бот `t.me/<бот>?start=<код>`: 32 случайных шестнадцатеричных
-- знака (122 бита случайности), не номер по порядку. Каждой строке — свой код: значение по
-- умолчанию вычисляется для каждой строки отдельно.
alter table parties add column bot_code text not null default replace(gen_random_uuid()::text, '-', '');
create unique index parties_bot_code_uq on parties (bot_code);

-- 2. Фирма не хочет сообщений о закупах (SPEC-32 §4).
alter table parties add column no_purchase_messages boolean not null default false;

-- 3. Руководитель среди людей фирмы: ему — сообщение о закупе и его имя в обращении.
alter table party_contacts add column is_head boolean not null default false;

-- 4. Строка книжки «Масло в коробке» (SPEC-32 §3): трансмиссионное масло больше не сбивает срок
-- масла двигателя. Категории масел с «трансмис», «короб» или «АКПП» в названии без заданной строки
-- получают её; прежние строки замен из таких категорий переходят в неё же.
alter table categories drop constraint categories_book_slot_check;
alter table categories add constraint categories_book_slot_check
    check (book_slot in ('none', 'oil', 'gear_oil', 'oil_filter', 'air_filter', 'cabin_filter', 'fuel_filter', 'other'));
alter table vehicle_book_intervals drop constraint vehicle_book_intervals_slot_check;
alter table vehicle_book_intervals add constraint vehicle_book_intervals_slot_check
    check (slot in ('oil', 'gear_oil', 'oil_filter', 'air_filter', 'cabin_filter', 'fuel_filter', 'other'));
alter table oil_change_items drop constraint oil_change_items_slot_check;
alter table oil_change_items add constraint oil_change_items_slot_check
    check (slot in ('oil', 'gear_oil', 'oil_filter', 'air_filter', 'cabin_filter', 'fuel_filter', 'other'));
update categories set book_slot = 'gear_oil'
where kind = 'oil' and book_slot is null
  and lower(name) similar to '%(трансмис|короб|акпп)%';
update oil_change_items i set slot = 'gear_oil'
from categories c
where c.id = i.category_id and c.book_slot = 'gear_oil' and i.slot = 'oil';

-- 5. Сообщения клиентам: очередь и журнал. Строка — одно сообщение одному получателю; состояние
-- меняет только отправка (ждёт → отправлено / ошибка). «Не отправлено» — у клиента нет ни бота,
-- ни WhatsApp. Текст и переменные шаблона хранятся как ушли.
create table client_messages (
    id          uuid primary key,
    branch_id   uuid not null references branches,
    party_id    uuid references parties,
    vehicle_id  uuid references party_vehicles,
    sale_id     uuid references sales,
    kind        text not null check (kind in ('oil', 'purchase')),
    -- «Отправить пробное» владельца: без окна отправки и без ключа повода.
    test        boolean not null default false,
    channel     text not null check (channel in ('telegram', 'whatsapp', 'none')),
    -- Чат Телеграма или телефон в международном виде.
    recipient   text not null default '',
    text        text not null,
    template    text not null default '',
    language    text not null default '',
    params      jsonb not null default '[]',
    status      text not null default 'pending'
                check (status in ('pending', 'sent', 'error', 'not_sent')),
    error       text not null default '',
    external_id text not null default '',
    attempts    integer not null default 0,
    next_try_at timestamptz not null default now(),
    user_id     uuid references users,
    created_at  timestamptz not null default now(),
    sent_at     timestamptz
);
create index client_messages_pending_idx on client_messages (next_try_at) where status = 'pending';
create index client_messages_branch_idx on client_messages (branch_id, created_at desc);

-- Поводы, по которым сообщение уже составлено: один повод — один раз (вид + клиент + машина +
-- цель или документ).
create table client_message_keys (
    key        text primary key,
    message_id uuid not null references client_messages,
    created_at timestamptz not null default now()
);

-- Сообщения о закупах — только о чеках после установки: прежние чеки не рассылаются.
insert into bot_state (key, value)
values ('client_messages_since', to_jsonb(to_char(now() at time zone 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"')))
on conflict (key) do nothing;
