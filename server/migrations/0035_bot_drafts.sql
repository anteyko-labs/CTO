-- Черновики операций владельца из бота (SPEC-24, ADR-068). Модель только предлагает черновик,
-- сервер проверяет его и показывает владельцу; проводится только кнопкой «✅ Провести».
-- Судьба черновика (проведён, отменён, истёк, отклонён) пишется один раз: после этого строка
-- не меняется и не удаляется.
create table bot_drafts (
    id          uuid primary key,
    chat_id     bigint not null,
    user_id     uuid not null references users,
    branch_id   uuid not null references branches,
    kind        text not null,
    payload     jsonb not null,
    summary     text not null,
    op_id       uuid not null unique,
    status      text not null default 'open'
                check (status in ('open', 'posted', 'cancelled', 'expired', 'rejected')),
    result      jsonb,
    created_at  timestamptz not null default now(),
    finished_at timestamptz,
    expires_at  timestamptz not null,
    check ((status = 'open') = (finished_at is null))
);
create index bot_drafts_chat_idx on bot_drafts (chat_id, created_at desc);
create index bot_drafts_posted_idx on bot_drafts (branch_id, finished_at) where status = 'posted';
-- У чата один открытый черновик: новый закрывает прежний.
create unique index bot_drafts_one_open on bot_drafts (chat_id) where status = 'open';

-- Открытый черновик можно только завершить (статус, итог, время завершения), завершённый —
-- ничем; удалить нельзя никакой.
create function bot_drafts_guard() returns trigger language plpgsql as $$
begin
    if tg_op = 'DELETE' then
        raise exception 'черновик бота не удаляется' using errcode = 'P0001';
    end if;
    if old.status <> 'open' or new.status = 'open'
       or (new.id, new.chat_id, new.user_id, new.branch_id, new.kind, new.payload, new.summary,
           new.op_id, new.created_at, new.expires_at)
          is distinct from
          (old.id, old.chat_id, old.user_id, old.branch_id, old.kind, old.payload, old.summary,
           old.op_id, old.created_at, old.expires_at) then
        raise exception 'завершённый черновик бота не меняется' using errcode = 'P0001';
    end if;
    return new;
end
$$;
create trigger bot_drafts_immutable before update or delete on bot_drafts
    for each row execute function bot_drafts_guard();
