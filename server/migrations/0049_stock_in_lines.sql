-- Оприходование — вариант прихода (SPEC-29 §8, ADR-086): один документ, несколько строк.
-- Шапка `stock_ins` (номер, комментарий, итог, сторно, кто, когда) остаётся; товар, количество
-- и цена переходят в строки. Строки не правятся, как и документ (инвариант 4).
create table stock_in_lines (
    id               uuid primary key,
    stock_in_id      uuid not null references stock_ins,
    line_no          integer not null check (line_no > 0),
    product_id       uuid not null references products,
    -- В единицах учёта: штуки, миллилитры, граммы; у сторно — с минусом.
    qty              bigint not null check (qty <> 0),
    -- Цена закупки за штуку, канистру масла или килограмм — как её вписали.
    price_tyiyn      bigint not null check (price_tyiyn >= 0),
    -- Стоимость строки `цена × количество / единица цены` (с минусом у сторно).
    cost_tyiyn       bigint not null,
    -- Цена продажи товара после этого оприходования, как у строки прихода; пусто — до 0049.
    sale_price_tyiyn bigint check (sale_price_tyiyn >= 0),
    unique (stock_in_id, line_no)
);
create index stock_in_lines_product_idx on stock_in_lines (product_id);
create trigger stock_in_lines_immutable before update or delete on stock_in_lines
    for each row execute function forbid_change();

-- Прежние оприходования и их сторно — по одной строке с теми же количеством, ценой и стоимостью:
-- история читается и сторнируется так же, как новые документы.
insert into stock_in_lines (id, stock_in_id, line_no, product_id, qty, price_tyiyn, cost_tyiyn)
select gen_random_uuid(), id, 1, product_id, qty, price_tyiyn, cost_tyiyn from stock_ins;

-- Товар, количество и цена теперь в строках; `cost_tyiyn` шапки — итог документа.
-- Проверка «сторно — с минусом» уходит вместе с `qty`: знак держат строки.
alter table stock_ins drop column product_id, drop column qty, drop column price_tyiyn;
