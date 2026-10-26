-- Следующая замена по каждой услуге чека (SPEC-34, ADR-109).

-- 1. Строки книжки «Антифриз» и «Тормозная жидкость».
alter table categories drop constraint categories_book_slot_check;
alter table categories add constraint categories_book_slot_check
    check (book_slot in ('none', 'oil', 'gear_oil', 'oil_filter', 'air_filter', 'cabin_filter', 'fuel_filter',
                         'antifreeze', 'brake_fluid', 'other'));
alter table vehicle_book_intervals drop constraint vehicle_book_intervals_slot_check;
alter table vehicle_book_intervals add constraint vehicle_book_intervals_slot_check
    check (slot in ('oil', 'gear_oil', 'oil_filter', 'air_filter', 'cabin_filter', 'fuel_filter',
                    'antifreeze', 'brake_fluid', 'other'));
alter table oil_change_items drop constraint oil_change_items_slot_check;
alter table oil_change_items add constraint oil_change_items_slot_check
    check (slot in ('oil', 'gear_oil', 'oil_filter', 'air_filter', 'cabin_filter', 'fuel_filter',
                    'antifreeze', 'brake_fluid', 'other'));

-- 2. Строка книжки услуги и её интервал. «Прочее» — услуга в книжку не пишется и не напоминает.
-- Интервал пустой — по строке книжки (по умолчанию).
alter table services add column book_slot text not null default 'other'
    check (book_slot in ('oil', 'gear_oil', 'oil_filter', 'air_filter', 'cabin_filter', 'fuel_filter',
                         'antifreeze', 'brake_fluid', 'other'));
alter table services add column book_interval_km integer check (book_interval_km between 500 and 200000);
alter table services add column book_interval_days integer check (book_interval_days between 1 and 1095);

-- Строка по названию услуги (то же правило — `service_slot_by_name` на сервере): сначала фильтры
-- (в «масляном фильтре» есть «масл»), потом антифриз, тормозная жидкость, коробка и масло мотора.
update services set book_slot = case
        when lower(name) like '%фильтр%' and lower(name) like '%воздуш%' then 'air_filter'
        when lower(name) like '%фильтр%' and lower(name) like '%салон%' then 'cabin_filter'
        when lower(name) like '%фильтр%' and lower(name) like '%топлив%' then 'fuel_filter'
        when lower(name) like '%фильтр%' and lower(name) like '%масл%' then 'oil_filter'
        when lower(name) like '%фильтр%' then 'other'
        when lower(name) similar to '%(антифриз|охлажд)%' then 'antifreeze'
        when lower(name) like '%тормоз%' then 'brake_fluid'
        when lower(name) similar to '%(коробк|акпп|мкпп|трансмис)%' then 'gear_oil'
        when lower(name) like '%масл%' then 'oil'
        else 'other'
    end;

-- 3. Цель следующей замены по строке записи: кассир задал её в чеке (`target_set`) — пробег (в
-- единице машины и в км) и дата по интервалу в днях; задал пустой — по строке не напоминать.
-- Без `target_set` срок, как раньше, — по интервалу.
alter table oil_change_items add column service_id uuid references services;
alter table oil_change_items add column target_set boolean not null default false;
alter table oil_change_items add column next_value integer check (next_value >= 0);
alter table oil_change_items add column next_unit text check (next_unit in ('km', 'mi'));
alter table oil_change_items add column next_km integer check (next_km >= 0);
alter table oil_change_items add column next_date date;
alter table oil_change_items add constraint oil_change_items_target_check
    check (target_set or (next_km is null and next_value is null and next_date is null));
