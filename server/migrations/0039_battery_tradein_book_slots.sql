-- Доработки по проверке на точке (SPEC-27, ADR-071): аккумуляторы и книжка по каждому фильтру.

-- Полярность — характеристика категорий аккумуляторов (SPEC-02). Категория, заведённая без
-- характеристик (например, сама при первом приёме на вес), получает её здесь.
update categories
set attributes = attributes || '[{"key": "polarity", "label": "Полярность", "type": "select",
                                  "options": ["прямая", "обратная"], "filterable": true}]'::jsonb
where kind = 'battery'
  and jsonb_typeof(attributes) = 'array'
  and not exists (select 1 from jsonb_array_elements(attributes) a where a ->> 'key' = 'polarity');

-- Старый аккумулятор в оплату чека: вес в граммах, сумма = вес × цена приёма за кг.
alter table sale_payments drop constraint sale_payments_method_check;
alter table sale_payments add constraint sale_payments_method_check
    check (method in ('cash', 'card', 'transfer', 'debt', 'bonus', 'battery'));
alter table sale_payments add column grams bigint;
alter table sale_payments add constraint sale_payments_battery_grams_check
    check ((method = 'battery') = (grams is not null));

-- Строка книжки у категории и интервал строки по умолчанию. Пусто — по виду категории, как
-- было: масла — строка масла, фильтры — масляного фильтра, остальное в книжку не идёт.
alter table categories add column book_slot text
    check (book_slot in ('none', 'oil', 'oil_filter', 'air_filter', 'cabin_filter', 'fuel_filter', 'other'));
alter table categories add column book_interval_km integer check (book_interval_km between 500 and 200000);
alter table categories add column book_interval_days integer check (book_interval_days between 1 and 1095);

-- Интервал строки у машины, если отличается от категории.
create table vehicle_book_intervals (
    vehicle_id    uuid not null references party_vehicles,
    branch_id     uuid not null references branches,
    slot          text not null
                  check (slot in ('oil', 'oil_filter', 'air_filter', 'cabin_filter', 'fuel_filter', 'other')),
    interval_km   integer not null check (interval_km between 500 and 200000),
    interval_days integer not null check (interval_days between 1 and 1095),
    updated_at    timestamptz not null default now(),
    primary key (vehicle_id, slot)
);

-- Прежний интервал машины (один на всю замену) — интервал масла и масляного фильтра этой машины,
-- если он не равен прежнему по умолчанию (8 000 км, 31 день). Колонки машины остаются для истории.
insert into vehicle_book_intervals (vehicle_id, branch_id, slot, interval_km, interval_days)
select v.id, v.branch_id, s.slot, least(greatest(v.interval_km, 500), 200000), v.interval_days
from party_vehicles v cross join (values ('oil'), ('oil_filter')) as s(slot)
where v.interval_km <> 8000 or v.interval_days <> 31;

-- Строки замены по слотам книжки.
create table oil_change_items (
    id            uuid primary key,
    branch_id     uuid not null references branches,
    oil_change_id uuid not null references oil_changes,
    slot          text not null
                  check (slot in ('oil', 'oil_filter', 'air_filter', 'cabin_filter', 'fuel_filter', 'other')),
    text          text not null,
    category_id   uuid references categories,
    position      integer not null default 0
);
create index oil_change_items_change_idx on oil_change_items (oil_change_id, position);

-- Прежние записи: масло — строка масла, фильтр — строка масляного фильтра. Тексты в
-- oil_changes остаются как были.
insert into oil_change_items (id, branch_id, oil_change_id, slot, text, position)
select gen_random_uuid(), branch_id, id, 'oil', oil_text, 0 from oil_changes where oil_text <> '';
insert into oil_change_items (id, branch_id, oil_change_id, slot, text, position)
select gen_random_uuid(), branch_id, id, 'oil_filter', filter_text, 1 from oil_changes where filter_text <> '';

-- Напоминания — по каждой строке книжки отдельно.
alter table oil_reminders add column slot text not null default 'oil';
alter table oil_reminders drop constraint oil_reminders_pkey;
alter table oil_reminders add primary key (vehicle_id, slot, due_date, days_before);
