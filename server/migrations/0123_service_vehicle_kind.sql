-- Задача 044: услуга — для легковых или грузовых; касса показывает услуги вида машины чека.
-- Все имеющиеся услуги — легковые.
alter table services add column vehicle_kind text not null default 'car'
    check (vehicle_kind in ('car', 'truck'));

-- Просьба владельца 21.11.2026: в Айкосе легковые услуги выключены, в остальных (Автодом) — грузовые.
insert into settings (branch_id, key, value)
select id, 'cashier', jsonb_build_object('services_car', false, 'vehicle_kind_default', 'truck')
from branches
where name ilike 'a%kos' or name ilike 'айкос'
on conflict (branch_id, key)
    do update set value = settings.value
        || jsonb_build_object('services_car', false, 'vehicle_kind_default', 'truck');

insert into settings (branch_id, key, value)
select id, 'cashier', jsonb_build_object('services_truck', false)
from branches
where not (name ilike 'a%kos' or name ilike 'айкос')
on conflict (branch_id, key)
    do update set value = settings.value || jsonb_build_object('services_truck', false);
