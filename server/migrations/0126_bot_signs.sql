-- Подключение клиента в боте по двум признакам из трёх: ИНН, ФИО/название, госномер (задача 046).

-- Чат ждёт сообщение с признаками после кнопки «🔎 Найти себя…».
create table bot_sign_wait (
    chat_id    bigint primary key,
    expires_at timestamptz not null
);

-- Неудачные попытки: не больше 5 за сутки на чат.
create table bot_sign_tries (
    chat_id    bigint not null,
    created_at timestamptz not null default now()
);
create index bot_sign_tries_chat_idx on bot_sign_tries (chat_id, created_at);
