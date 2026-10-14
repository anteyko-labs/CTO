-- Вопросы владельца боту обычным текстом (SPEC-21, ADR-060).

-- Каждый вопрос: что спросили, какие отчёты открыты, сколько ушло токенов, ответ или ошибка.
-- Записи не правятся и не удаляются — как журнал действий.
create table bot_questions (
    id            uuid primary key,
    chat_id       bigint not null,
    user_id       uuid not null references users,
    branch_id     uuid not null references branches,
    asked_at      timestamptz not null default now(),
    question      text not null,
    tools         jsonb not null default '[]',
    input_tokens  bigint not null default 0,
    output_tokens bigint not null default 0,
    answer        text,
    error         text
);
create index bot_questions_branch_idx on bot_questions (branch_id, asked_at desc);
create trigger bot_questions_immutable before update or delete on bot_questions
    for each row execute function forbid_change();

-- Память разговора: последние сообщения чата за 30 минут, старше — удаляются.
create table bot_dialog (
    id        uuid primary key,
    chat_id   bigint not null,
    branch_id uuid not null references branches,
    at        timestamptz not null default now(),
    role      text not null check (role in ('user', 'assistant')),
    content   text not null
);
create index bot_dialog_chat_idx on bot_dialog (chat_id, at desc);
