-- Журнал пересчёта строки ревизии (задача 003, Н14): порядок записей — порядок вставки под
-- замком черновика (`seq`), а не время начала транзакции; `seq` же — версия строки для экрана:
-- ответ записи или перечитывания показывается, только если он новее уже показанного.
-- Время записи — момент вставки (`clock_timestamp`): «ещё считают: Нурлан 20 с назад».
alter table revision_line_adds add column seq bigserial;
alter table revision_line_adds alter column created_at set default clock_timestamp();
create index revision_line_adds_seq on revision_line_adds (revision_id, product_id, seq);
