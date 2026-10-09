-- Порог подарка (SPEC-11): подарок положен, когда товара-условия в чеке не меньше
-- `min_units` в его единицах учёта (мл у масла, штуки у остального). 0 — с любого количества.
alter table gift_rules add column min_units bigint not null default 0 check (min_units >= 0);
