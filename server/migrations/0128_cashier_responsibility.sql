-- Чек юрлица под ответственность кассира (задача 046, ADR-213): пояснение кассира к запросу
-- подтверждения фирмы, проведённому до её ответа. Кто и когда — уже в forced_by и forced_at.
alter table sale_approvals add column forced_note text;
-- Метка «Под ответственность: Имя» в чеке, истории и карточке фирмы — из журнала по чеку.
create index audit_log_responsibility_idx on audit_log (entity_id)
    where action = 'sale.cashier_responsibility';
