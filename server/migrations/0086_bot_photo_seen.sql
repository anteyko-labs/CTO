-- Повтор того же фото накладной в бот (задача 003): Телеграм может прислать обновление ещё раз,
-- а человек — переслать то же фото. В течение суток то же фото в том же чате — не новый черновик,
-- а ответ «это фото уже принято». Ключ фото — `file_unique_id` Телеграма (без него — `file_id`).
-- Таблица служебная, как `bot_state`: запись занимает фото до создания черновика и указывает на
-- него после; через сутки то же фото снова заводит черновик.
create table bot_photo_seen (
    chat_id    bigint not null,
    file_key   text not null check (length(file_key) between 1 and 256),
    branch_id  uuid not null references branches,
    -- Пусто — черновик ещё сохраняется (фото только что пришло).
    draft_id   uuid references receipt_drafts,
    created_at timestamptz not null default now(),
    primary key (chat_id, file_key)
);
