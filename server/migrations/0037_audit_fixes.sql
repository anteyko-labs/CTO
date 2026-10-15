-- Исправления по ревью бота, брака и поставок (ADR-070).

-- Владелец, который привязал чат: его номер в Телеграме. «✅ Провести» и «✖ Отмена» под
-- черновиком принимаются только от него (SPEC-24). Бот отвечает только в личных чатах, а там
-- номер чата совпадает с номером пользователя — им и заполняем прежние привязки.
alter table telegram_links add column tg_user_id bigint;
update telegram_links set tg_user_id = chat_id where chat_id > 0;

-- Поставки поставщика по товарам и сторно прихода ищут строки по товару и накладные
-- поставщика (SPEC-26).
create index receipt_lines_product_idx on receipt_lines (product_id);
create index receipts_branch_supplier_idx on receipts (branch_id, supplier_id);
