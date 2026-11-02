-- Килограммы у любого товара; лом аккумуляторов — по пометке, а не по единице «г» (ADR-174).
-- Прежде карточкой лома считался первый не архивный товар в граммах: с обычными товарами в кг
-- это правило путало бы приём аккумуляторов с ними.
alter table products add column system_role text
    check (system_role in ('battery_scrap'));

-- Одна карточка на роль.
create unique index products_system_role_key on products (system_role) where system_role is not null;

-- Пометка существующей карточке лома — по прежнему правилу.
update products set system_role = 'battery_scrap'
where id = (select id from products where unit = 'g' and not archived order by created_at, id limit 1);

-- Смена единицы шт ↔ кг и л ↔ кг (только без остатка) — в журнале пересчётов.
alter table product_unit_changes drop constraint product_unit_changes_from_unit_check;
alter table product_unit_changes add constraint product_unit_changes_from_unit_check
    check (from_unit in ('piece', 'ml', 'g'));
alter table product_unit_changes drop constraint product_unit_changes_to_unit_check;
alter table product_unit_changes add constraint product_unit_changes_to_unit_check
    check (to_unit in ('piece', 'ml', 'g'));
