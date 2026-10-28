-- Приёмка товара на планшете (SPEC-39, ADR-134). Накладная, подготовленная человеком с правом
-- «Приход товара», ждёт приёмки и ничего не проводит; кассир на планшете отмечает принятое,
-- выбирает себя, вводит PIN и расписывается — приход проводится по принятому одной транзакцией.
-- Администратор присылает фото накладных в бот (ADR-135): его чат — в той же `telegram_links`,
-- новых таблиц для этого нет.

-- 1. Черновик фото «на приёмке» (`acceptance`): из «Накладных по фото» его не провести и не
--    убрать, пока приёмка ждёт; приход по приёмке — «использован», приёмку сняли — снова «новый».
alter table receipt_drafts drop constraint receipt_drafts_status_check;
alter table receipt_drafts add constraint receipt_drafts_status_check
    check (status in ('new', 'acceptance', 'used', 'removed'));
do $$
declare
    c text;
begin
    -- Проверка «новый ⇔ не закрыт» из 0058 без имени: имя дала база, ищем её по тексту.
    for c in
        select conname from pg_constraint
        where conrelid = 'public.receipt_drafts'::regclass and contype = 'c'
          and pg_get_constraintdef(oid) like '%(status = ''new''::text) = (closed_at IS NULL)%'
    loop
        execute format('alter table public.receipt_drafts drop constraint %I', c);
    end loop;
end
$$;
alter table receipt_drafts add constraint receipt_drafts_open_check
    check ((status in ('new', 'acceptance')) = (closed_at is null)
           and (closed_at is null) = (closed_by is null));

-- Черновик не удаляется; меняются только итог распознавания (один раз), статус и перенос фото в
-- накладную. «На приёмке» — ещё открытый черновик: из него — в «новый» или «использован».
create or replace function receipt_drafts_guard() returns trigger language plpgsql as $$
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
    if old.status not in ('new', 'acceptance')
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

-- 2. Приёмка: что отправили (поставщик, накладная, строки по накладной) и итог — кто принял,
--    подпись, приход. Запись не удаляется и меняется один раз: из «ждёт» в итог.
create table receipt_acceptances (
    id               uuid primary key,
    branch_id        uuid not null references branches,
    -- Накладная по фото, из которой подготовили приёмку; её фото переходит к приходу.
    draft_id         uuid references receipt_drafts,
    supplier_id      uuid not null references suppliers,
    supplier_doc     text not null default '' check (char_length(supplier_doc) <= 200),
    comment          text not null default '' check (char_length(comment) <= 500),
    -- waiting — ждёт приёмки; accepted — принята, приход проведён; refused — кассир отказался
    -- от приёмки; cancelled — убрали с приёмки в программе.
    status           text not null default 'waiting'
                     check (status in ('waiting', 'accepted', 'refused', 'cancelled')),
    -- Кто отправил: приход проводится от его имени (SPEC-39 §4).
    sent_by          uuid not null references users,
    sent_device_id   uuid not null,
    sent_at          timestamptz not null default now(),
    -- Отправили кнопкой «📦 На приёмку» в боте — чат.
    sent_chat_id     bigint,
    closed_at        timestamptz,
    -- Пользователь и устройство планшета (принял, отказ) или кто убрал с приёмки.
    closed_by        uuid references users,
    closed_device_id uuid,
    -- Кассир, подтвердивший приёмку или отказ своим PIN.
    employee_id      uuid references employees,
    reason           text check (char_length(reason) <= 500),
    accept_comment   text not null default '' check (char_length(accept_comment) <= 500),
    -- Подпись кассира: PNG до 200 КБ, только у принятой (ADR-080, ADR-134).
    signature_png    bytea check (octet_length(signature_png) between 8 and 204800),
    receipt_id       uuid unique references receipts,
    check ((status = 'waiting') = (closed_at is null)),
    check ((closed_at is null) = (closed_by is null)
           and (closed_at is null) = (closed_device_id is null)),
    check ((status = 'accepted') = (receipt_id is not null)),
    check ((status = 'accepted') = (signature_png is not null)),
    check (status not in ('accepted', 'refused') or employee_id is not null),
    check (status not in ('refused', 'cancelled') or reason is not null)
);
create index receipt_acceptances_branch_idx on receipt_acceptances (branch_id, status, sent_at desc);
-- Одна накладная (поставщик и номер) и один черновик фото не ждут приёмки дважды.
create unique index receipt_acceptances_doc_waiting
    on receipt_acceptances (branch_id, supplier_id, lower(supplier_doc))
    where status = 'waiting' and supplier_doc <> '';
create unique index receipt_acceptances_draft_waiting
    on receipt_acceptances (draft_id) where status = 'waiting';

-- Строки накладной: количество в единицах учёта (штуки, мл, граммы) и сумма строки. Итог строки
-- пишется один раз при приёмке: принято, другой товар той же единицы, причина, комментарий.
create table receipt_acceptance_lines (
    acceptance_id       uuid not null references receipt_acceptances,
    line_no             integer not null check (line_no > 0),
    product_id          uuid not null references products,
    invoice_qty         bigint not null check (invoice_qty > 0),
    line_cost_tyiyn     bigint not null check (line_cost_tyiyn >= 0),
    sale_price_tyiyn    bigint check (sale_price_tyiyn >= 0),
    accepted_qty        bigint check (accepted_qty >= 0),
    accepted_product_id uuid references products,
    -- Почему принято меньше: не привезли, брак или повреждено (на склад не идёт), другое.
    reason              text check (reason in ('missing', 'damaged', 'other')),
    comment             text not null default '' check (char_length(comment) <= 500),
    primary key (acceptance_id, line_no)
);
create index receipt_acceptance_lines_product_idx on receipt_acceptance_lines (product_id);

-- Привезли, но этого нет в накладной: в приход не входит, только в уведомление владельцу.
create table receipt_acceptance_extras (
    acceptance_id uuid not null references receipt_acceptances,
    no            integer not null check (no between 1 and 50),
    name          text not null check (char_length(name) between 1 and 120),
    barcode       text not null default '' check (char_length(barcode) <= 64),
    -- Товар учёта по коду со сканера, если такой есть.
    product_id    uuid references products,
    qty           bigint not null check (qty > 0 and qty <= 1000000),
    primary key (acceptance_id, no)
);

create function receipt_acceptances_guard() returns trigger language plpgsql as $$
begin
    if tg_op = 'DELETE' then
        raise exception 'приёмка не удаляется' using errcode = 'P0001';
    end if;
    if old.status <> 'waiting' then
        raise exception 'приёмка уже закрыта' using errcode = 'P0001';
    end if;
    if (new.id, new.branch_id, new.draft_id, new.supplier_id, new.supplier_doc, new.comment,
        new.sent_by, new.sent_device_id, new.sent_at, new.sent_chat_id)
       is distinct from
       (old.id, old.branch_id, old.draft_id, old.supplier_id, old.supplier_doc, old.comment,
        old.sent_by, old.sent_device_id, old.sent_at, old.sent_chat_id) then
        raise exception 'приёмка не меняется: уберите её и отправьте заново' using errcode = 'P0001';
    end if;
    return new;
end
$$;
create trigger receipt_acceptances_immutable before update or delete on receipt_acceptances
    for each row execute function receipt_acceptances_guard();

create function receipt_acceptance_lines_guard() returns trigger language plpgsql as $$
begin
    if tg_op = 'DELETE' then
        raise exception 'строка приёмки не удаляется' using errcode = 'P0001';
    end if;
    if (new.acceptance_id, new.line_no, new.product_id, new.invoice_qty, new.line_cost_tyiyn,
        new.sale_price_tyiyn)
       is distinct from
       (old.acceptance_id, old.line_no, old.product_id, old.invoice_qty, old.line_cost_tyiyn,
        old.sale_price_tyiyn) then
        raise exception 'строка накладной не меняется' using errcode = 'P0001';
    end if;
    if old.accepted_qty is not null then
        raise exception 'принятое по строке уже записано' using errcode = 'P0001';
    end if;
    if (select a.status from public.receipt_acceptances a where a.id = old.acceptance_id) <> 'waiting' then
        raise exception 'приёмка уже закрыта' using errcode = 'P0001';
    end if;
    return new;
end
$$;
create trigger receipt_acceptance_lines_immutable before update or delete on receipt_acceptance_lines
    for each row execute function receipt_acceptance_lines_guard();

create trigger receipt_acceptance_extras_immutable before update or delete on receipt_acceptance_extras
    for each row execute function forbid_change();

-- 3. Право «Приёмка товара (планшет)» (SPEC-39 §8). Список прав в проверке дополняется тем, что
--    в ней уже есть: права, добавленные другими обновлениями, не теряются при любом их порядке.
do $$
declare
    def  text;
    keys text[];
begin
    select pg_get_constraintdef(oid) into def from pg_constraint
    where conrelid = 'public.user_permissions'::regclass
      and conname = 'user_permissions_permission_check';
    select coalesce(array_agg(t.m[1]), '{}') into keys
    from regexp_matches(coalesce(def, ''), '''([a-z_]+)''', 'g') as t(m);
    if not ('acceptance' = any (keys)) then
        keys := keys || 'acceptance'::text;
    end if;
    alter table public.user_permissions drop constraint if exists user_permissions_permission_check;
    execute format(
        'alter table public.user_permissions add constraint user_permissions_permission_check check (permission in (%s))',
        (select string_agg(quote_literal(k), ', ') from unnest(keys) as k));
end
$$;

-- Существующим администраторам — сразу, как в ADR-097: без подготовленной накладной, PIN и
-- подписи кассира право ничего не проводит. Касса-моноблок сможет принимать товар сразу.
insert into user_permissions (user_id, branch_id, permission, granted_by)
select u.id, u.branch_id, 'acceptance', null
from users u
where u.role = 'admin'
on conflict do nothing;
