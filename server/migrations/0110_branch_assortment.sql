-- Задача 036: свой ассортимент филиала. Карточка товара общая (I16), у филиала — отметка
-- «Продаётся в филиале». Существующие строки — во всех филиалах, как было.
alter table branch_products add column listed boolean not null default true;
-- «Свой список товаров» филиала: новый товар другого филиала в нём не продаётся. Без отметки —
-- как было: новый товар — во всех таких филиалах.
alter table branches add column own_assortment boolean not null default false;
