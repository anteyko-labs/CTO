-- Телеграм-бот владельца (SPEC-18, ADR-050): привязка чата по одноразовому коду, курсор уведомлений.
create table telegram_links (
    chat_id   bigint primary key,
    user_id   uuid not null unique references users,
    branch_id uuid not null references branches,
    linked_at timestamptz not null default now()
);

create table telegram_codes (
    code       text primary key,
    user_id    uuid not null references users,
    expires_at timestamptz not null
);

-- Служебное состояние бота: смещение обновлений Телеграма и до какой записи журнала дошли.
create table bot_state (
    key   text primary key,
    value jsonb not null
);
