-- Доработки по проверке на точке, часть 3 (SPEC-29, ADR-079, ADR-080).

-- 1. Фото накладной (ADR-080): файлы лежат в базе — копия базы (pg_dump) уносит их вместе с
-- приходом, новых служб нет. Один файл — не больше 5 МБ, только снимки и PDF. Файл не правится
-- и не удаляется: владелец может только скрыть его с причиной (отметка ставится один раз).
create table receipt_files (
    id            uuid primary key,
    branch_id     uuid not null references branches,
    receipt_id    uuid not null references receipts,
    -- Повтор той же загрузки (обрыв связи) не заводит второй файл.
    op_id         uuid not null unique,
    name          text not null default '',
    content_type  text not null
                  check (content_type in ('image/jpeg', 'image/png', 'image/webp', 'application/pdf')),
    size_bytes    integer not null check (size_bytes > 0 and size_bytes <= 5242880),
    data          bytea not null,
    user_id       uuid not null references users,
    device_id     uuid not null,
    created_at    timestamptz not null default now(),
    hidden_at     timestamptz,
    hidden_by     uuid references users,
    hidden_reason text,
    check ((hidden_at is null) = (hidden_by is null) and (hidden_at is null) = (hidden_reason is null)),
    check (octet_length(data) = size_bytes)
);
create index receipt_files_receipt_idx on receipt_files (receipt_id, created_at);

create function receipt_files_guard() returns trigger language plpgsql as $$
begin
    if tg_op = 'DELETE' then
        raise exception 'фото накладной не удаляется' using errcode = 'P0001';
    end if;
    if old.hidden_at is not null then
        raise exception 'фото накладной уже скрыто' using errcode = 'P0001';
    end if;
    if (new.id, new.branch_id, new.receipt_id, new.op_id, new.name, new.content_type, new.size_bytes,
        new.data, new.user_id, new.device_id, new.created_at)
       is distinct from
       (old.id, old.branch_id, old.receipt_id, old.op_id, old.name, old.content_type, old.size_bytes,
        old.data, old.user_id, old.device_id, old.created_at) then
        raise exception 'фото накладной не меняется' using errcode = 'P0001';
    end if;
    return new;
end
$$;
create trigger receipt_files_immutable before update or delete on receipt_files
    for each row execute function receipt_files_guard();

-- 2. Оприходование без поставщика (SPEC-29 §8): товар пришёл без накладной и без денег.
-- Количество и стоимость — по цене закупки, которую вписали; сторно — строка с обратными
-- количеством и стоимостью и ссылкой на исходную.
create table stock_ins (
    id          uuid primary key,
    branch_id   uuid not null references branches,
    number      bigint not null,
    product_id  uuid not null references products,
    qty         bigint not null check (qty <> 0),
    -- Цена закупки за штуку, канистру масла или килограмм — как её вписали.
    price_tyiyn bigint not null check (price_tyiyn >= 0),
    -- Стоимость всего количества (с минусом у сторно).
    cost_tyiyn  bigint not null,
    comment     text not null check (btrim(comment) <> ''),
    reversal_of uuid unique references stock_ins,
    user_id     uuid not null references users,
    device_id   uuid not null,
    created_at  timestamptz not null default now(),
    unique (branch_id, number),
    check ((reversal_of is null) = (qty > 0))
);
create index stock_ins_branch_idx on stock_ins (branch_id, created_at desc);
create index stock_ins_product_idx on stock_ins (product_id);
create trigger stock_ins_immutable before update or delete on stock_ins
    for each row execute function forbid_change();

-- 3. Возврат товара поставщику, не брак (SPEC-29 §9). Товар уходит со склада по средней
-- себестоимости (инвариант 3); сумма возврата договорная. Итог — меньше наш долг поставщику
-- (`debt`) или деньги от него в выбранный счёт (`money`). Разница «сумма − себестоимость» —
-- строка прибыли «Возврат поставщику: разница». Сторно — документ с обратными суммами.
create table supplier_returns (
    id           uuid primary key,
    branch_id    uuid not null references branches,
    number       bigint not null,
    supplier_id  uuid not null references suppliers,
    settle       text not null check (settle in ('debt', 'money')),
    account_id   uuid references cash_accounts,
    amount_tyiyn bigint not null,
    cost_tyiyn   bigint not null,
    diff_tyiyn   bigint not null,
    comment      text not null default '',
    reversal_of  uuid unique references supplier_returns,
    user_id      uuid not null references users,
    device_id    uuid not null,
    created_at   timestamptz not null default now(),
    unique (branch_id, number),
    check ((settle = 'money') = (account_id is not null)),
    check (diff_tyiyn = amount_tyiyn - cost_tyiyn)
);
create index supplier_returns_branch_idx on supplier_returns (branch_id, created_at desc);
create index supplier_returns_supplier_idx on supplier_returns (branch_id, supplier_id);
create trigger supplier_returns_immutable before update or delete on supplier_returns
    for each row execute function forbid_change();

create table supplier_return_lines (
    id           uuid primary key,
    return_id    uuid not null references supplier_returns,
    line_no      integer not null,
    product_id   uuid not null references products,
    qty          bigint not null,
    cost_tyiyn   bigint not null,
    amount_tyiyn bigint not null,
    unique (return_id, line_no)
);
create index supplier_return_lines_product_idx on supplier_return_lines (product_id);
create trigger supplier_return_lines_immutable before update or delete on supplier_return_lines
    for each row execute function forbid_change();

-- Возврат в счёт долга закрывает долги поставщику, как оплата (ADR-073): запись долга с плюсом,
-- `doc_type = 'supplier_return'`. Его сторно и снятый сторно возврат в распределении не участвуют.
create or replace function supplier_ledger_live(e party_ledger) returns boolean language sql stable as $$
    select e.amount_tyiyn <> 0
       and e.reversal_of is null
       and not exists (select 1 from party_ledger x where x.reversal_of = e.id)
       and e.doc_type not in ('receipt_reversal', 'defect_reversal', 'supplier_return_reversal')
       and not (e.doc_type = 'receipt'
                and exists (select 1 from receipts r where r.reversal_of = e.doc_id))
       and not (e.doc_type = 'defect'
                and exists (select 1 from defect_resolutions d where d.reversal_of = e.doc_id))
       and not (e.doc_type = 'supplier_return'
                and exists (select 1 from supplier_returns s where s.reversal_of = e.doc_id))
$$;
