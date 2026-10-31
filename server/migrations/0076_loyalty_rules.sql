-- Баллы по товару, категории и общей ставке (задача 002, П1; ADR-140).
-- По умолчанию всё как раньше: товар и категория — «как выше», общая ставка из настройки
-- `loyalty` (0,5 %), начисление — только в чеке с заменой масла (ADR-124).

-- 1. Правило баллов у товара и у категории.
--    inherit — товар: как в категории; категория: по общей ставке;
--    none    — не начислять;
--    percent — процент от оплаченного деньгами за строку, сотые доли процента (50 = 0,5 %);
--    fixed   — баллы в тыйынах за единицу: штуку, литр (товар в мл) или кг (товар в граммах).
alter table products
    add column points_mode text not null default 'inherit'
        check (points_mode in ('inherit', 'none', 'percent', 'fixed')),
    add column points_value bigint not null default 0
        check (points_value >= 0 and points_value <= 100000000),
    add constraint products_points_percent check (points_mode <> 'percent' or points_value <= 10000);

alter table categories
    add column points_mode text not null default 'inherit'
        check (points_mode in ('inherit', 'none', 'percent', 'fixed')),
    add column points_value bigint not null default 0
        check (points_value >= 0 and points_value <= 100000000),
    add constraint categories_points_percent check (points_mode <> 'percent' or points_value <= 10000);

-- 2. Начисление по строкам чека: из чего сложилась запись `accrual` и что сняла запись
--    `accrual_back` возврата. `sale_id` и `line_no` — строка исходного чека. Возврат строки
--    снимает баллы этой строки (I4: новой записью, исходная не меняется).
create table loyalty_lines (
    ledger_id    uuid not null references loyalty_ledger,
    sale_id      uuid not null references sales,
    line_no      integer not null check (line_no > 0),
    amount_tyiyn bigint not null check (amount_tyiyn <> 0),
    -- Откуда правило: product — карточка товара, category — категория, common — общая ставка.
    source       text not null check (source in ('product', 'category', 'common')),
    primary key (ledger_id, line_no)
);
create index loyalty_lines_sale_idx on loyalty_lines (sale_id, line_no);
create trigger loyalty_lines_immutable before update or delete on loyalty_lines
    for each row execute function forbid_change();
