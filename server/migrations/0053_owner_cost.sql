-- Цена закупки, которую указал владелец без прихода (ADR-062): при заведении товара или для
-- товара, проданного без цены. Сторно последней закупки больше не обнуляет «последнюю
-- закупочную», если цена владельца есть: берётся она (ADR-092).
alter table branch_products
    add column owner_cost_qty bigint not null default 0 check (owner_cost_qty >= 0),
    add column owner_cost_tyiyn bigint not null default 0 check (owner_cost_tyiyn >= 0);

-- Цены владельца из журнала: последняя по каждому товару.
update branch_products bp
   set owner_cost_qty = (a.data ->> 'per')::bigint,
       owner_cost_tyiyn = (a.data ->> 'price_tyiyn')::bigint
  from (select distinct on (branch_id, entity_id) branch_id, entity_id, data
          from audit_log
         where action = 'stock.purchase_price' and entity_id is not null
           and data ? 'per' and data ? 'price_tyiyn'
         order by branch_id, entity_id, created_at desc, id desc) a
 where bp.branch_id = a.branch_id and bp.product_id = a.entity_id
   and (a.data ->> 'per')::bigint > 0 and (a.data ->> 'price_tyiyn')::bigint > 0;

-- Цена при заведении товара в журнал не попадала: у товара без единой закупки «последняя
-- закупочная» и есть цена владельца.
update branch_products bp
   set owner_cost_qty = bp.last_cost_qty, owner_cost_tyiyn = bp.last_cost_tyiyn
 where bp.owner_cost_qty = 0 and bp.last_cost_qty > 0 and bp.last_cost_tyiyn > 0
   and not exists (select 1 from stock_movements m
                    where m.branch_id = bp.branch_id and m.product_id = bp.product_id
                      and m.doc_type in ('receipt', 'stock_in', 'battery_intake', 'battery_tradein',
                                         'oil_transfer'));
