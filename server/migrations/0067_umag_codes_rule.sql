-- Коды строки Умага по итогам проверки 26.10.2026 (SPEC-35 §1, ADR-113). «Доп. код» выгрузки —
-- свободный текст: кроме кодов в нём слова и знаки («SHELLHELIX CITY 5W30 1L», «0888602305  U»), а
-- прежняя выгрузка обрезала его на 100 знаках — обрывок EAN («…;46») становился общим кодом двух
-- разных строк, и товар одной строки забирал другую (касса продавала не тот товар).
--
-- Вызовы своих функций внутри функций — со схемой `public.`: копия базы (pg_dump) восстанавливается
-- с пустым search_path, и без схемы вычисляемые столбцы `old_catalog` не создаются
-- (deploy/check-restore.sh).

-- 1. Кусок «доп. кода» — код, если похож на код: латиница, цифры и дефис, не меньше четырёх
-- цифр, с буквами — от шести знаков, не длиннее кода товара (64); вязкость («10W-40») — не код.
create function umag_extra_code_ok(t text) returns boolean
    language sql immutable parallel safe as $$
    select t ~ '^[0-9A-Za-z-]+$' and length(t) <= 64
       and length(regexp_replace(t, '[^0-9]', '', 'g')) >= 4
       and (t ~ '^[0-9]+$' or length(t) >= 6)
       and t !~* '^[0-9]{1,2}w-?[0-9]{2}$'
$$;

-- Артикул — код, если в нём латиница, цифры и дефис, от четырёх знаков и есть цифра («BKR6E»).
create function umag_article_ok(t text) returns boolean
    language sql immutable parallel safe as $$
    select t ~ '^[0-9A-Za-z-]+$' and length(t) between 4 and 64 and t ~ '[0-9]'
$$;

-- Числовой код без потерянных Excel нулей впереди; остальные — как есть.
create function umag_bare(c text) returns text
    language sql immutable parallel safe as $$
    select case when c ~ '^[0-9]+$' and ltrim(c, '0') <> '' then ltrim(c, '0') else c end
$$;

create function umag_bare_codes(cs text[]) returns text[]
    language sql immutable parallel safe as $$
    select coalesce(array_agg(public.umag_bare(t.c) order by t.n), '{}') from unnest(cs) with ordinality as t(c, n)
$$;

-- 2. Коды строки — заново: вычисляемые столбцы пересоздаются по новому правилу.
alter table old_catalog drop column codes, drop column keys;

-- Прежняя выгрузка обрезала «доп. код» на 100 знаках: последний кусок такой строки — обрывок.
update old_catalog set extra_code = regexp_replace(extra_code, '[;,[:space:]]*[^;,[:space:]]*$', '')
where length(extra_code) >= 100;

create or replace function umag_codes(barcode text, article text, extra_code text) returns text[]
    language sql immutable parallel safe as $$
    select coalesce(array_agg(x.c order by x.n), '{}')
    from (select t.c, min(t.n) as n
          from unnest(array[btrim(coalesce(barcode, ''))]
                      || case when public.umag_article_ok(btrim(coalesce(article, '')))
                              then array[btrim(article)] else '{}'::text[] end
                      || array(select e.c
                               from unnest(regexp_split_to_array(btrim(coalesce(extra_code, '')),
                                                                 '[;,[:space:]]+')) with ordinality as e(c, k)
                               where public.umag_extra_code_ok(e.c)
                               order by e.k))
               with ordinality as t(c, n)
          where t.c <> ''
          group by t.c) x
$$;

alter table old_catalog
    add column codes text[] not null generated always as (umag_codes(barcode, article, extra_code)) stored,
    add column keys text[] not null generated always as (umag_codes(barcode, '', extra_code)) stored,
    -- Ключи без нулей впереди: строку «00R12» находит по индексу и код товара «0R12».
    add column keys_bare text[] not null
        generated always as (umag_bare_codes(umag_codes(barcode, '', extra_code))) stored,
    -- Название для точного сравнения при автосвязывании.
    add column name_norm text not null generated always as (umag_norm_name(name)) stored;
create index old_catalog_keys_bare_idx on old_catalog using gin (keys_bare) with (fastupdate = off);
create index old_catalog_name_norm_idx on old_catalog (branch_id, name_norm);

-- 3. Строку Умага продавали мерой (литр, кг, метр): её коды Умага продавали литр или килограмм,
-- а у товара учёта (канистра, ведро) такой код добавил бы на кассе всю тару. Товару переходят
-- только заводские коды (ADR-113).
create function umag_by_measure(unit text) returns boolean
    language sql immutable parallel safe as $$
    select lower(btrim(coalesce(unit, ''))) ~ '^(л|лт|литр[а-я]*|кг|килограмм[а-я]*|г|гр|грамм[а-я]*|м|метр[а-я]*)\.?$'
$$;

-- Код меры строки «литр», «кг», «метр» — не заводской код: короче восьми цифр (276…, PLU) или
-- EAN-13 магазина с префиксом 2: весовая наклейка «1 литр» (код строки 2760275, количество
-- 01000 и контрольная — 2760275010002) и коды магазина Умага 211…. Числовой — и с потерянными
-- нулями впереди (повторная проверка рабочей выгрузки 26.10.2026, ADR-113).
create function umag_measure_code(c text, unit text) returns boolean
    language sql immutable parallel safe as $$
    select public.umag_by_measure(unit) and public.umag_bare(btrim(coalesce(c, ''))) ~ '^([0-9]{1,7}|2[0-9]{12})$'
$$;

create function umag_product_codes(codes text[], unit text) returns text[]
    language sql immutable parallel safe as $$
    select coalesce(array_agg(t.c order by t.n), '{}')
    from unnest(codes) with ordinality as t(c, n)
    where not public.umag_measure_code(t.c, unit)
$$;

-- 4. Коды товаров, перенесённые из Умага по прежнему правилу и не коды по новому: слова и знаки
-- («CITY», «-», «1L»), обрывки («46»), коды меры строк «литр» (и наклейка «1 литр»). У товара
-- остаётся хотя бы один код.
with junk as (
    select b.code, b.product_id from product_barcodes b
    where b.source = 'umag'
      and ((not umag_extra_code_ok(b.code)
            and not exists (select 1 from old_catalog o where btrim(o.barcode) = b.code)
            and not exists (select 1 from old_catalog o
                            where btrim(o.article) = b.code and umag_article_ok(b.code)))
           or exists (select 1 from old_catalog o
                      where o.product_id = b.product_id and b.code = any(o.codes)
                        and umag_measure_code(b.code, o.unit)))
)
delete from product_barcodes b using junk j
where b.code = j.code
  and exists (select 1 from product_barcodes c
              where c.product_id = b.product_id
                and not exists (select 1 from junk k where k.code = c.code));

-- Отметки «конфликт кода» — заново, по переносимым кодам.
update old_catalog o set code_conflicts = coalesce((
       select jsonb_agg(jsonb_build_object('code', c.code, 'product_id', x.product_id,
                                           'product_name', p.name) order by c.n)
       from unnest(umag_product_codes(o.codes, o.unit)) with ordinality as c(code, n)
       cross join lateral (select y.product_id from code_products(c.code) y
                           where y.product_id <> o.product_id
                           order by (y.code = c.code) desc, y.code limit 1) x
       join products p on p.id = x.product_id
       where not exists (select 1 from product_barcodes s
                         where s.code = c.code and s.product_id = o.product_id)), '[]'::jsonb)
where o.product_id is not null;

-- Статистика списка — сразу: без неё автосвязывание по названию сравнивало строки перебором
-- (секунды под замком филиала), пока не придёт автоматический пересчёт.
analyze old_catalog;
analyze product_barcodes;
