-- Исправления цепочек склада по ревью (ADR-054).

-- Слияния до 0013 не помнили основную карточку: возвраты и сторно уходили на архивную.
-- Восстанавливаем по журналу слияний, а где его нет — по движениям «merge» (doc_id — основная).
update products p set merged_into = (a.entity_id)
from audit_log a
where a.action = 'product.merge' and a.entity_id is not null
  and (a.data->>'from')::uuid = p.id and p.merged_into is null and p.archived;

update products p set merged_into = m.doc_id
from stock_movements m
where m.doc_type = 'merge' and m.product_id = p.id and m.qty_delta < 0
  and p.merged_into is null and p.archived and m.doc_id <> p.id;

-- Правила подарков влитых карточек — на основную (если у основной своего правила нет).
update gift_rules g set trigger_product_id = p.merged_into
from products p
where p.id = g.trigger_product_id and p.merged_into is not null
  and not exists (select 1 from gift_rules x where x.branch_id = g.branch_id and x.trigger_product_id = p.merged_into);
update gift_rules g set active = false
from products p
where p.id = g.trigger_product_id and p.merged_into is not null;
delete from gift_rule_items i using products p
where p.id = i.gift_product_id and p.merged_into is not null
  and exists (select 1 from gift_rule_items x where x.rule_id = i.rule_id and x.gift_product_id = p.merged_into);
update gift_rule_items i set gift_product_id = p.merged_into
from products p
where p.id = i.gift_product_id and p.merged_into is not null;

-- Ревизия: остаток на момент пересчёта строки. Расхождение = пересчитано − было тогда,
-- а продажи и приходы между пересчётом и проведением остаются в учёте.
alter table revision_lines add column stock_at_count bigint;
