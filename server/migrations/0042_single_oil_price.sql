-- Одна цена масла (ADR-074). Масло всегда в таре (`container_ml` обязателен, products_check),
-- его цена — цена канистры (бочки); розлив за литр считается от неё на сервере
-- (domain::money::canister_pour_price) и не хранится. Прежние цены розлива остаются в журнале.
insert into audit_log (id, branch_id, user_id, action, entity, entity_id, data)
select gen_random_uuid(), bp.branch_id, null, 'product.pour_price_derived', 'product', p.id,
       jsonb_build_object(
           'name', p.name,
           'old_pour', bp.pour_price_per_l_tyiyn,
           'sale', bp.sale_price_tyiyn,
           'container_ml', p.container_ml)
from branch_products bp
join products p on p.id = bp.product_id
where bp.pour_price_per_l_tyiyn is not null;

-- Колонка остаётся пустой, а не удаляется: прежняя версия программы (откат обновления на точке)
-- её читает. Записать в неё цену больше нельзя.
update branch_products set pour_price_per_l_tyiyn = null where pour_price_per_l_tyiyn is not null;
alter table branch_products add constraint branch_products_no_pour_price
    check (pour_price_per_l_tyiyn is null);
