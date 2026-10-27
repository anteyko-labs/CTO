-- Коды из старой базы Умаг — коды товара (SPEC-35, ADR-110): строка Умага ссылается на товар
-- учёта, все её коды переносятся в коды товара; код, занятый другим товаром, не переносится —
-- у строки отметка конфликта.

-- 1. Коды строки Умага: штрихкод, артикул (если похож на код: цифры, латиница и дефис, от 4
-- знаков), каждый «доп. код» (через «;», «,» или пробел). Пустые и повторы выкидываются, порядок —
-- как в строке. Одно правило для поиска, связывания и переноса — столбец `codes`.
create function umag_codes(barcode text, article text, extra_code text) returns text[]
    language sql immutable parallel safe as $$
    select coalesce(array_agg(x.c order by x.n), '{}')
    from (select t.c, min(t.n) as n
          from unnest(array[btrim(coalesce(barcode, ''))]
                      || case when btrim(coalesce(article, '')) ~ '^[0-9A-Za-z-]+$'
                                   and length(btrim(article)) >= 4
                              then array[btrim(article)] else '{}'::text[] end
                      || regexp_split_to_array(btrim(coalesce(extra_code, '')), '[;,[:space:]]+'))
               with ordinality as t(c, n)
          where t.c <> ''
          group by t.c) x
$$;

alter table old_catalog
    add column codes text[] not null generated always as (umag_codes(barcode, article, extra_code)) stored,
    -- Коды, по которым строку узнают (автосвязывание, «уже заведена», новая выгрузка): штрихкод
    -- и «доп. коды». Без артикула: он бывает общим у разных товаров.
    add column keys text[] not null generated always as (umag_codes(barcode, '', extra_code)) stored,
    -- Товар учёта, с которым связана строка (SPEC-35 §1); товары общие для филиалов.
    add column product_id uuid references products,
    -- Коды строки, занятые другими товарами: [{code, product_id, product_name}].
    add column code_conflicts jsonb not null default '[]';
create index old_catalog_product_idx on old_catalog (product_id) where product_id is not null;
-- Индексы списка — без отложенной вставки (fastupdate): после выгрузки и автосвязывания похожие
-- пары и коды сразу ищутся по индексу, а не перебором тысяч отложенных записей (минута на запрос).
create index old_catalog_keys_idx on old_catalog using gin (keys) with (fastupdate = off);
alter index old_catalog_name_trgm_idx set (fastupdate = off);

-- Есть ли у строки Умага код: тот же или тот же числовой без потерянных Excel нулей впереди.
create function umag_has_code(codes text[], c text) returns boolean
    language sql immutable parallel safe as $$
    select c = any(codes)
        or (c ~ '^[0-9]+$' and ltrim(c, '0') <> ''
            and exists (select 1 from unnest(codes) x where x ~ '^[0-9]+$' and ltrim(x, '0') = ltrim(c, '0')))
$$;

-- Название для точного сравнения при автосвязывании: регистр, «ё»→«е», без кавычек, пробелы
-- схлопнуты.
create function umag_norm_name(t text) returns text language sql immutable parallel safe as $$
    select btrim(regexp_replace(translate(lower(coalesce(t, '')), 'ё"«»''“”„`’', 'е'), '\s+', ' ', 'g'))
$$;

-- Ключ строки Умага, переживающий новую выгрузку: штрихкод, без него — название. Вызов своей
-- функции — со схемой: копия базы восстанавливается с пустым search_path (deploy/check-restore.sh).
create function umag_key(barcode text, name text) returns text language sql immutable parallel safe as $$
    select case when btrim(coalesce(barcode, '')) <> '' then btrim(barcode)
                else 'name:' || public.umag_norm_name(name) end
$$;

-- 2. «Не то» в похожих парах (SPEC-35 §3): пара больше не предлагается и не связывается
-- автоматически. Строка — по ключу `umag_key` (обычно штрихкод), список Умага у филиала свой.
create table old_catalog_link_rejects (
    branch_id    uuid not null references branches,
    umag_barcode text not null,
    product_id   uuid not null references products,
    primary key (branch_id, umag_barcode, product_id)
);

-- 3. Откуда код товара: свой (скан, ввод, выданный программой) или перенесён из Умага.
alter table product_barcodes add column source text not null default 'own'
    check (source in ('own', 'umag'));

-- 4. Числовой код находит товар и без нулей впереди, потерянных Excel (SPEC-35 §1).
create index product_barcodes_digits_idx on product_barcodes (ltrim(code, '0')) where code ~ '^[0-9]+$';

-- Товары с этим кодом: тот же код, а у числового — и тот же без нулей впереди. `internal` — код
-- выдан программой (ADR-026): по нему строку Умага с товаром не связывают — в Умаге свои коды
-- магазина могут совпасть с нашими.
create function code_products(c text) returns table (product_id uuid, code text, internal boolean)
    language sql stable parallel safe as $$
    select b.product_id, b.code, b.internal from product_barcodes b where b.code = c
    union all
    select b.product_id, b.code, b.internal from product_barcodes b
    where c ~ '^[0-9]+$' and ltrim(c, '0') <> ''
      and b.code ~ '^[0-9]+$' and ltrim(b.code, '0') = ltrim(c, '0') and b.code <> c
$$;
