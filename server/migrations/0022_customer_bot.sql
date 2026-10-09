-- Масляная книжка частного клиента в боте и напоминания о замене (SPEC-16, SPEC-18, ADR-051).

-- Интервал по дате — в днях, по умолчанию 31 день (решение заказчика).
alter table party_vehicles add column interval_days integer not null default 31 check (interval_days between 1 and 730);
alter table party_vehicles drop column interval_months;

-- Клиент, подключившийся к боту своим номером телефона.
create table telegram_customers (
    chat_id   bigint primary key,
    party_id  uuid not null references parties,
    phone     text not null,
    linked_at timestamptz not null default now()
);
create index telegram_customers_party_idx on telegram_customers (party_id);

-- Какие напоминания уже ушли: одно на срок замены и на «за 7 дней» / «за 1 день».
create table oil_reminders (
    vehicle_id uuid not null references party_vehicles,
    due_date   date not null,
    days_before integer not null,
    sent_at    timestamptz not null default now(),
    primary key (vehicle_id, due_date, days_before)
);
