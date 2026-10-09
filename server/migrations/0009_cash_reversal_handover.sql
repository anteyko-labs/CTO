-- Сторно движений и перемещений кассы, сдача кассы после закрытия смены (SPEC-05, ADR-037).

-- Внесение или изъятие сторнируется один раз.
create unique index cash_movements_reversal_once on cash_movements (doc_id)
    where kind = 'reversal' and doc_type = 'cash_movement';
-- Перемещение сторнируется один раз.
create unique index cash_transfers_reversal_once on cash_transfers (reversal_of)
    where reversal_of is not null;

-- Что сделали с деньгами после закрытия: сколько ушло в сейф, сколько осталось на размен.
-- Одна запись на закрытие; после переоткрытия и нового закрытия — новая.
create table shift_handovers (
    id            uuid primary key,
    shift_id      uuid not null references shifts,
    close_id      uuid not null unique references shift_closes,
    transfer_id   uuid references cash_transfers,
    to_safe_tyiyn bigint not null check (to_safe_tyiyn >= 0),
    left_tyiyn    bigint not null,
    user_id       uuid not null references users,
    device_id     uuid not null,
    created_at    timestamptz not null default now()
);

create trigger shift_handovers_immutable before update or delete on shift_handovers
    for each row execute function forbid_change();
create trigger shift_reopens_immutable before update or delete on shift_reopens
    for each row execute function forbid_change();

-- Расход сторнируется один раз, даже при двух одновременных запросах (SPEC-06).
create unique index expenses_reversal_once on expenses (reversal_of) where reversal_of is not null;
