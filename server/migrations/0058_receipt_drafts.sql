-- Накладная по фото (SPEC-30 §9, ADR-098): фото из чата владельца или с экрана «Новый приход»
-- сохраняется черновиком прихода и распознаётся внешним сервисом. Черновик только подсказывает:
-- приход проводит человек. Фото лежит в базе, как фото накладной (ADR-080); после проведения
-- прихода оно переносится в `receipt_files`, а здесь остаётся ссылка на файл.
create table receipt_drafts (
    id             uuid primary key,
    branch_id      uuid not null references branches,
    -- Откуда фото: бот (чат владельца) или экран «Новый приход».
    source         text not null check (source in ('bot', 'screen')),
    -- Повтор той же загрузки с экрана (обрыв связи) не заводит второй черновик.
    op_id          uuid unique,
    chat_id        bigint,
    name           text not null default '',
    content_type   text not null
                   check (content_type in ('image/jpeg', 'image/png', 'image/webp', 'application/pdf')),
    size_bytes     integer not null check (size_bytes > 0 and size_bytes <= 5242880),
    -- Пусто — фото уже перенесено в накладную (`file_id`).
    data           bytea,
    -- Распознавание: off — не настроено, limit — лимит часа, pending — идёт, done, failed.
    recognition    text not null check (recognition in ('off', 'limit', 'pending', 'done', 'failed')),
    -- Ответ сервиса, проверенный сервером: поставщик, номер, дата, строки, итог.
    recognized     jsonb,
    error          text,
    input_tokens   bigint not null default 0,
    output_tokens  bigint not null default 0,
    status         text not null default 'new' check (status in ('new', 'used', 'removed')),
    receipt_id     uuid references receipts,
    file_id        uuid references receipt_files,
    user_id        uuid not null references users,
    device_id      uuid not null,
    created_at     timestamptz not null default now(),
    closed_at      timestamptz,
    closed_by      uuid references users,
    check ((data is null) = (file_id is not null)),
    check (data is null or octet_length(data) = size_bytes),
    check ((status = 'used') = (receipt_id is not null)),
    check ((status = 'new') = (closed_at is null) and (closed_at is null) = (closed_by is null)),
    check ((recognition = 'done') = (recognized is not null))
);
create index receipt_drafts_branch_idx on receipt_drafts (branch_id, status, created_at desc);
-- Общий лимит распознаваний и вопросов в час на филиал (ADR-098).
create index receipt_drafts_recognition_idx on receipt_drafts (branch_id, created_at)
    where recognition in ('pending', 'done', 'failed');

-- Черновик не удаляется; меняются только итог распознавания (один раз, пока идёт), статус
-- (новый → использован или убран) и перенос фото в накладную вместе с использованием.
create function receipt_drafts_guard() returns trigger language plpgsql as $$
begin
    if tg_op = 'DELETE' then
        raise exception 'черновик прихода не удаляется' using errcode = 'P0001';
    end if;
    if (new.id, new.branch_id, new.source, new.op_id, new.chat_id, new.name, new.content_type,
        new.size_bytes, new.user_id, new.device_id, new.created_at)
       is distinct from
       (old.id, old.branch_id, old.source, old.op_id, old.chat_id, old.name, old.content_type,
        old.size_bytes, old.user_id, old.device_id, old.created_at) then
        raise exception 'черновик прихода не меняется' using errcode = 'P0001';
    end if;
    if (new.recognition, new.recognized, new.error, new.input_tokens, new.output_tokens)
       is distinct from
       (old.recognition, old.recognized, old.error, old.input_tokens, old.output_tokens)
       and old.recognition <> 'pending' then
        raise exception 'распознавание черновика уже записано' using errcode = 'P0001';
    end if;
    if old.status <> 'new'
       and (new.status, new.receipt_id, new.file_id, new.data, new.closed_at, new.closed_by)
           is distinct from
           (old.status, old.receipt_id, old.file_id, old.data, old.closed_at, old.closed_by) then
        raise exception 'черновик прихода уже закрыт' using errcode = 'P0001';
    end if;
    if new.data is distinct from old.data and not (new.data is null and new.status = 'used') then
        raise exception 'фото черновика не меняется' using errcode = 'P0001';
    end if;
    return new;
end
$$;
create trigger receipt_drafts_immutable before update or delete on receipt_drafts
    for each row execute function receipt_drafts_guard();
