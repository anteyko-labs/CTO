-- Свои строки книжки точки (задача 016, ADR-184): работа вне встроенных строк («Свечи», «Ремень
-- ГРМ») — строка `other` со ссылкой на свою строку; срок — по её интервалу (пустой — без срока).
-- Встроенные строки и их CHECK (0064) не меняются.
create table book_custom_slots (
    id            uuid primary key,
    branch_id     uuid not null references branches,
    name          text not null,
    interval_km   integer check (interval_km between 500 and 200000),
    interval_days integer check (interval_days between 1 and 1095),
    hidden        boolean not null default false,
    created_at    timestamptz not null default now()
);
create unique index book_custom_slots_name_idx on book_custom_slots (branch_id, lower(name));

alter table oil_change_items add column custom_slot_id uuid references book_custom_slots;
alter table oil_change_items add constraint oil_change_items_custom_slot_check
    check (custom_slot_id is null or slot = 'other');
