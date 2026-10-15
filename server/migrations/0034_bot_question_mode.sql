-- Режим «💬 Спросить» в чате владельца (SPEC-21, ADR-067): только пока он включён, свободный
-- текст владельца уходит в сервис разбора вопросов. Строка — режим включали в этом чате;
-- действует 10 минут после последнего сообщения (`last_at`), «✖ Закончить» удаляет строку.
create table bot_question_mode (
    chat_id    bigint primary key,
    user_id    uuid not null references users,
    branch_id  uuid not null references branches,
    started_at timestamptz not null default now(),
    last_at    timestamptz not null default now()
);
