-- Администратор привязывает Телеграм сам (SPEC-40 §10, ADR-172): кто выдал код — сам
-- администратор (`by_self`; владелец выключил настройку — такие коды удаляются той же
-- транзакцией) и Телеграм-имя привязанного чата для показа «к кому привязан».
alter table telegram_codes add column by_self boolean not null default false;
alter table telegram_links add column tg_name text;
