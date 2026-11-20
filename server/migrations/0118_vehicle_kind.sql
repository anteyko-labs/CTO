-- Задача 040 A, ADR-205: «Грузовая / Легковая» на каждом чеке — только отметка (цены и состав не
-- меняются). Значение нового чека — настройка кассы филиала `vehicle_kind_default`.
alter table sales add column vehicle_kind text not null default 'car'
    check (vehicle_kind in ('car', 'truck'));

-- Айкос — грузовые по умолчанию (просьба владельца 20.11.2026); остальные — легковые (как было).
insert into settings (branch_id, key, value)
select id, 'cashier', jsonb_build_object('vehicle_kind_default', 'truck')
from branches
where name ilike 'a%kos' or name ilike 'айкос'
on conflict (branch_id, key)
    do update set value = settings.value || jsonb_build_object('vehicle_kind_default', 'truck');
