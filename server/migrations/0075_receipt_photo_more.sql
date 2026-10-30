-- Приход по фото: второе фото и запомненные товары (задача 002, П2–П3; SPEC-30 §9, SPEC-03).

-- 1. «Ещё фото» в открытом черновике: фото дописывает тот же черновик. Дополнительное фото —
--    черновик со ссылкой на первый (`parent_id`); в «Накладных по фото» виден только первый, его
--    строки — строки всех фото. При проведении фото всех черновиков переходят к накладной.
alter table receipt_drafts add column parent_id uuid references receipt_drafts;
alter table receipt_drafts add constraint receipt_drafts_parent_check check (parent_id <> id);
create index receipt_drafts_parent_idx on receipt_drafts (parent_id, created_at)
    where parent_id is not null;

-- Ссылка на первый черновик ставится при записи и не меняется (остальное — как в 0074).
create or replace function receipt_drafts_guard() returns trigger language plpgsql as $$
begin
    if tg_op = 'DELETE' then
        raise exception 'черновик прихода не удаляется' using errcode = 'P0001';
    end if;
    if (new.id, new.branch_id, new.source, new.op_id, new.chat_id, new.name, new.content_type,
        new.size_bytes, new.user_id, new.device_id, new.created_at, new.parent_id)
       is distinct from
       (old.id, old.branch_id, old.source, old.op_id, old.chat_id, old.name, old.content_type,
        old.size_bytes, old.user_id, old.device_id, old.created_at, old.parent_id) then
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

-- 2. Запомненный выбор: «название строки у поставщика → товар». В следующий раз такая строка
--    этого поставщика совпадает точно. Запоминает филиал (инвариант 16): у филиалов свои
--    приёмщики и свои привычки выбора, а поставщики и каталог общие. Это справочник, а не
--    документ: запись заменяется новым выбором и удаляется; всё — в журнале действий.
create table supplier_line_map (
    id           uuid primary key,
    branch_id    uuid not null references branches,
    supplier_id  uuid not null references suppliers,
    -- Название строки, как в накладной, и ключ сравнения: строчные буквы, пробелы схлопнуты.
    name         text not null check (char_length(name) between 1 and 200),
    name_key     text not null check (char_length(name_key) between 1 and 200),
    product_id   uuid not null references products,
    op_id        uuid not null,
    user_id      uuid not null references users,
    created_at   timestamptz not null default now(),
    updated_at   timestamptz not null default now(),
    unique (branch_id, supplier_id, name_key)
);
create index supplier_line_map_supplier_idx on supplier_line_map (supplier_id, branch_id);
