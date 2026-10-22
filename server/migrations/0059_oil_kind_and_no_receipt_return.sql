-- Бесплатная замена по виду масла и возврат без чека (SPEC-30 §15–16, ADR-104, ADR-105).

-- 1. Категория масла у услуги «бесплатно с нашим маслом» (ADR-104): услуга бесплатна, только если
-- в чеке нашего масла этой категории не меньше канистры. Без категории флажок не действует.
alter table services add column oil_category_id uuid references categories;

-- Категория по названию услуги: «АКПП», «короб», «трансмис» — трансмиссионная, иначе — моторная.
-- Моторной без такого названия считается единственная категория масла, если она не трансмиссионная.
-- Флажки, поставленные по слову «масл», владелец проверяет сам на экране «Услуги».
with gear as (
    select id from categories
    where kind = 'oil' and name ilike any (array['%трансмис%', '%Трансмис%', '%ТРАНСМИС%'])
    order by created_at, id limit 1
), motor as (
    select coalesce(
        (select id from categories
         where kind = 'oil' and name ilike any (array['%мотор%', '%Мотор%', '%МОТОР%'])
         order by created_at, id limit 1),
        (select min(id::text)::uuid from categories
         where kind = 'oil' and name not ilike all (array['%трансмис%', '%Трансмис%', '%ТРАНСМИС%'])
         having count(*) = 1)) as id
)
update services s
set oil_category_id = case
        when s.name ilike any (array['%акпп%', '%АКПП%', '%Акпп%', '%короб%', '%Короб%', '%КОРОБ%',
                                     '%трансмис%', '%Трансмис%', '%ТРАНСМИС%'])
            then (select id from gear)
        else (select id from motor)
    end
where s.free_with_oil;

-- 2. Удержание за замену при возврате масла (ADR-104): строка возврата с ценой услуги, которая
-- перестала быть бесплатной. `withheld_for` — номер строки услуги в исходном чеке; удерживается
-- один раз на строку (проверяет сервер под блокировкой исходного чека).
alter table sale_lines add column withheld_for integer;

-- 3. Возврат без чека (ADR-105): возврат без исходного чека, только владелец.
alter table sales add column no_receipt boolean not null default false;
alter table sales drop constraint sales_check;
alter table sales add constraint sales_check check (
    (kind = 'return') = (reversal_of is not null or no_receipt)
    and not (no_receipt and reversal_of is not null)
);
