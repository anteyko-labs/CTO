-- «Изменить оплату» чека и «из наличных в QR» за смену (SPEC-37, ADR-132).
--
-- Пересчитанные наличные не трогаем: если наличные уже вошли в пересчёт «Кассы» (закрытие смены
-- или открытие следующей с «Пересчитал»), остаток кассы не меняется, а меняется расхождение этого
-- пересчёта — движение «пересчёт» с документом смены. Здесь — запись изменения: что поменяли,
-- чей пересчёт поправлен и на сколько, что вернули или удержали с кассира. Записи только
-- добавляются; отмена переноса — новая запись со ссылкой на исходную.

-- Перенос «из наличных в QR»: «Касса» −, счёт банка +.
alter table cash_movements drop constraint cash_movements_kind_check;
alter table cash_movements add constraint cash_movements_kind_check
    check (kind in ('sale', 'sale_return', 'cash_in', 'cash_out', 'transfer_in', 'transfer_out',
                    'bank_fee', 'expense', 'payout', 'supplier_payment', 'debt_repayment',
                    'count_diff', 'reversal', 'battery_intake', 'supplier_refund', 'cash_to_bank'));

create table pay_fixes (
    id                uuid primary key,
    branch_id         uuid not null references branches,
    -- Повтор того же «Сохранить» или «Перенести» не проводит второй раз.
    op_id             uuid not null unique,
    -- `sale` — изменение оплаты чека, `shift` — перенос из наличных в QR за смену.
    kind              text not null check (kind in ('sale', 'shift')),
    -- Изменение оплаты: исправление (сторно и новый чек одной операцией, ADR-085).
    correction_id     uuid references doc_corrections,
    sale_id           uuid references sales,
    reversal_sale_id  uuid references sales,
    new_sale_id       uuid references sales,
    -- Перенос: смена, счёт банка, сумма и комиссия (у отмены — с минусом).
    shift_id          uuid references shifts,
    account_id        uuid references cash_accounts,
    amount_tyiyn      bigint not null default 0,
    fee_tyiyn         bigint not null default 0,
    -- Чей пересчёт поправлен: смена и когда пересчитали (при открытии или при закрытии);
    -- пусто — наличные ещё не пересчитаны, изменился остаток «Кассы».
    counted_shift_id  uuid references shifts,
    counted_at        text check (counted_at in ('open', 'close')),
    -- Изменение наличных: новые − прежние (у переноса — минус сумма).
    cash_delta_tyiyn  bigint not null default 0,
    -- Расхождение пересчёта до и после (< 0 — недостача).
    diff_before_tyiyn bigint,
    diff_after_tyiyn  bigint,
    count_movement_id uuid references cash_movements,
    -- Поправка удержания с кассира: > 0 возвращено, < 0 удержано.
    withhold_tyiyn    bigint not null default 0,
    accrual_id        uuid references payroll_accruals,
    comment           text not null check (btrim(comment) <> '' and char_length(comment) <= 500),
    -- Отмена переноса — один раз.
    reversal_of       uuid unique references pay_fixes,
    user_id           uuid not null references users,
    device_id         uuid not null,
    created_at        timestamptz not null default now(),
    check ((counted_shift_id is null) = (counted_at is null)),
    check (kind <> 'sale' or (correction_id is not null and new_sale_id is not null)),
    check (kind <> 'shift' or (shift_id is not null and account_id is not null))
);
create index pay_fixes_branch_idx on pay_fixes (branch_id, created_at desc);
create index pay_fixes_counted_idx on pay_fixes (counted_shift_id) where counted_shift_id is not null;
create index pay_fixes_shift_idx on pay_fixes (shift_id) where shift_id is not null;
create index pay_fixes_reversal_sale_idx on pay_fixes (reversal_sale_id) where reversal_sale_id is not null;
create index pay_fixes_new_sale_idx on pay_fixes (new_sale_id) where new_sale_id is not null;
create index pay_fixes_count_move_idx on pay_fixes (count_movement_id) where count_movement_id is not null;
create index pay_fixes_accrual_idx on pay_fixes (accrual_id) where accrual_id is not null;
create trigger pay_fixes_immutable before update or delete on pay_fixes
    for each row execute function forbid_change();

-- Документы изменения для уже пересчитанных наличных относятся к смене пересчёта (блок «После
-- закрытия»), а не к смене, открытой в момент изменения: её отчёт, «Должно быть» и прибыль их не
-- видят (SPEC-37 §4). Чек — сторно или новый чек такого изменения.
create function public.pay_fix_counted_sale(p_sale uuid) returns boolean
language sql stable as $$
    select exists (select 1 from public.pay_fixes f
                   where f.counted_shift_id is not null
                     and (f.reversal_sale_id = p_sale or f.new_sale_id = p_sale))
$$;

-- Движение денег такого изменения: деньги сторно и нового чека, перенос, его комиссия и поправка
-- пересчёта.
create function public.pay_fix_counted_move(p_id uuid, p_doc_type text, p_doc_id uuid) returns boolean
language sql stable as $$
    select exists (select 1 from public.pay_fixes f
                   where f.counted_shift_id is not null
                     and ((p_doc_type = 'sale' and (f.reversal_sale_id = p_doc_id or f.new_sale_id = p_doc_id))
                          or (p_doc_type = 'pay_fix' and f.id = p_doc_id)
                          or f.count_movement_id = p_id))
$$;

-- Начисление такого изменения: процент и ставки по сторно и новому чеку, поправка удержания и
-- её сторно.
create function public.pay_fix_counted_accrual(p_id uuid, p_doc_type text, p_doc_id uuid) returns boolean
language sql stable as $$
    select (p_doc_type in ('sale', 'sale_return') and public.pay_fix_counted_sale(p_doc_id))
        or exists (select 1 from public.pay_fixes f
                   where f.counted_shift_id is not null
                     and (f.accrual_id = p_id or (p_doc_type = 'reversal' and f.accrual_id = p_doc_id)))
$$;
