-- Кросс-номера фильтров (SPEC-17) и адрес доставки в чеке (SPEC-04, доставка бесплатная).
create table product_cross_numbers (
    product_id uuid not null references products,
    code       text not null,
    code_norm  text not null,
    primary key (product_id, code_norm)
);
create index product_cross_numbers_norm_idx on product_cross_numbers (code_norm);

-- Нормализованный номер: без пробелов, дефисов, точек и косых, в верхнем регистре.
create function norm_code(t text) returns text language sql immutable as $$
    select upper(regexp_replace(coalesce(t, ''), '[^0-9A-Za-zА-Яа-яЁё]', '', 'g'))
$$;

alter table sales add column delivery_address text not null default '';
