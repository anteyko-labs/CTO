-- Доставка событий журнала в чаты владельца по каждому чату отдельно (SPEC-18, ADR-063).
-- Курсор `bot_state.cursor` стал нижней границей: события после него ищутся с запасом 15 минут
-- назад (запись журнала получает время начала транзакции и может появиться позже), а что уже
-- ушло в чат — отмечено здесь. Отметки старше двух дней удаляются: окно поиска не длиннее суток.
create table telegram_sent (
    chat_id  bigint not null,
    audit_id uuid not null,
    sent_at  timestamptz not null default now(),
    primary key (chat_id, audit_id)
);
create index telegram_sent_at_idx on telegram_sent (sent_at);

-- Неверные коды привязки владельца: не больше 5 на чат и 20 всего за 15 минут (ADR-063).
-- Код общий для всех филиалов, поэтому неверный код к филиалу не отнести — предел общий.
create table telegram_code_failures (
    chat_id bigint not null,
    at      timestamptz not null default now()
);
create index telegram_code_failures_at_idx on telegram_code_failures (at);
create index telegram_code_failures_chat_idx on telegram_code_failures (chat_id, at);

-- Длинный список машин в боте: остальные открываются номером — ожидание ввода «find» (SPEC-22).
alter table bot_input drop constraint bot_input_kind_check;
alter table bot_input add constraint bot_input_kind_check check (kind in ('plate', 'brand', 'note', 'find'));
