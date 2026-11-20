-- Задача 041 A: все фильтры владельца в книжке — новые строки книжки; «Куда поставили» у строки
-- фильтра в чеке (масляный — как фильтр коробки, воздушный — как салонный).
alter table categories drop constraint categories_book_slot_check;
alter table categories add constraint categories_book_slot_check
    check (book_slot in ('none', 'oil', 'gear_oil', 'oil_filter', 'air_filter', 'cabin_filter', 'fuel_filter',
                         'gearbox_filter', 'fuel_separator', 'water_separator', 'coolant_filter',
                         'hydraulic_filter', 'antifreeze', 'brake_fluid', 'other'));
alter table vehicle_book_intervals drop constraint vehicle_book_intervals_slot_check;
alter table vehicle_book_intervals add constraint vehicle_book_intervals_slot_check
    check (slot in ('oil', 'gear_oil', 'oil_filter', 'air_filter', 'cabin_filter', 'fuel_filter',
                    'gearbox_filter', 'fuel_separator', 'water_separator', 'coolant_filter',
                    'hydraulic_filter', 'antifreeze', 'brake_fluid', 'other'));
alter table oil_change_items drop constraint oil_change_items_slot_check;
alter table oil_change_items add constraint oil_change_items_slot_check
    check (slot in ('oil', 'gear_oil', 'oil_filter', 'air_filter', 'cabin_filter', 'fuel_filter',
                    'gearbox_filter', 'fuel_separator', 'water_separator', 'coolant_filter',
                    'hydraulic_filter', 'antifreeze', 'brake_fluid', 'other'));
alter table services drop constraint services_book_slot_check;
alter table services add constraint services_book_slot_check
    check (book_slot in ('oil', 'gear_oil', 'oil_filter', 'air_filter', 'cabin_filter', 'fuel_filter',
                         'gearbox_filter', 'fuel_separator', 'water_separator', 'coolant_filter',
                         'hydraulic_filter', 'antifreeze', 'brake_fluid', 'other'));

-- «Куда поставили» строки чека: пусто — строка категории.
alter table sale_lines add column book_slot text
    check (book_slot in ('gearbox_filter', 'cabin_filter'));

update categories set book_slot = 'gearbox_filter' where lower(name) = lower('ФИЛЬТР АКПП');

-- Недостающие категории фильтров; интервалы пустые — владелец ставит в категории.
insert into categories (id, name, kind, book_slot)
select gen_random_uuid(), n.name, 'filter', n.slot
from (values ('СЕПАРАТОРЫ', 'fuel_separator'), ('ВЛАГООТДЕЛИТЕЛИ', 'water_separator'),
             ('ФИЛЬТРЫ АНТИФРИЗА', 'coolant_filter'), ('ГИДРАВЛИЧЕСКИЕ ФИЛЬТРЫ', 'hydraulic_filter')) as n(name, slot)
where not exists (select 1 from categories c where lower(c.name) = lower(n.name));
