-- Брак и возврат поставщику (SPEC-25, ADR-069).
-- Отметка брака снимает товар из продаваемого остатка движением `defect` по средней
-- себестоимости; стоимость фиксируется в записи. Решение владельца — отдельная запись,
-- её отмена — сторно со ссылкой. У записи брака меняются только статус и время решения.

create table defects (
    id          uuid primary key,
    branch_id   uuid not null references branches,
    number      bigint not null,
    product_id  uuid not null references products,
    qty         bigint not null check (qty > 0),
    cost_tyiyn  bigint not null,
    reason      text not null check (btrim(reason) <> ''),
    supplier_id uuid references suppliers,
    user_id     uuid not null references users,
    device_id   uuid not null,
    created_at  timestamptz not null default now(),
    -- pending — ждёт решения; written_off — списан; returned — возвращён поставщику;
    -- cancelled — отметку сняли, товар вернулся в остаток.
    status      text not null default 'pending'
                check (status in ('pending', 'written_off', 'returned', 'cancelled')),
    resolved_at timestamptz,
    check ((status = 'pending') = (resolved_at is null)),
    unique (branch_id, number)
);
create index defects_branch_idx on defects (branch_id, status, created_at desc);
create index defects_product_idx on defects (product_id);

-- Проведённая отметка не правится: меняются только статус и время решения; удалить нельзя.
create function defects_guard() returns trigger language plpgsql as $$
begin
    if tg_op = 'DELETE' then
        raise exception 'запись брака не удаляется' using errcode = 'P0001';
    end if;
    if (new.id, new.branch_id, new.number, new.product_id, new.qty, new.cost_tyiyn, new.reason,
        new.supplier_id, new.user_id, new.device_id, new.created_at)
       is distinct from
       (old.id, old.branch_id, old.number, old.product_id, old.qty, old.cost_tyiyn, old.reason,
        old.supplier_id, old.user_id, old.device_id, old.created_at) then
        raise exception 'запись брака не меняется' using errcode = 'P0001';
    end if;
    return new;
end
$$;
create trigger defects_immutable before update or delete on defects
    for each row execute function defects_guard();

-- Решение владельца. Отмена решения — строка с обратными суммами и ссылкой на исходную.
-- writeoff_tyiyn — убыток списания (строка прибыли «Списание брака»);
-- diff_tyiyn — возмещение минус стоимость (строка «Брак: разница возмещения», минус — убыток).
create table defect_resolutions (
    id              uuid primary key,
    branch_id       uuid not null references branches,
    defect_id       uuid not null references defects,
    kind            text not null check (kind in ('write_off', 'replace', 'credit', 'cash')),
    supplier_id     uuid references suppliers,
    replace_qty     bigint,
    replace_tyiyn   bigint not null default 0,
    refund_tyiyn    bigint,
    account_id      uuid references cash_accounts,
    writeoff_tyiyn  bigint not null default 0,
    diff_tyiyn      bigint not null default 0,
    comment         text not null default '',
    reversal_of     uuid unique references defect_resolutions,
    user_id         uuid not null references users,
    device_id       uuid not null,
    created_at      timestamptz not null default now()
);
create index defect_resolutions_defect_idx on defect_resolutions (defect_id, created_at);
create index defect_resolutions_branch_idx on defect_resolutions (branch_id, created_at);
create trigger defect_resolutions_immutable before update or delete on defect_resolutions
    for each row execute function forbid_change();

-- Деньги от поставщика за брак приходят в кассу движением `supplier_refund`.
alter table cash_movements drop constraint cash_movements_kind_check;
alter table cash_movements add constraint cash_movements_kind_check
    check (kind in ('sale', 'sale_return', 'cash_in', 'cash_out', 'transfer_in', 'transfer_out',
                    'bank_fee', 'expense', 'payout', 'supplier_payment', 'debt_repayment',
                    'count_diff', 'reversal', 'battery_intake', 'supplier_refund'));
