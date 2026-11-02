-- Метры у любого товара (задача 006, ADR-175): остаток в миллиметрах, цена — за метр (1000 мм),
-- как килограммы в граммах (ADR-174). Строка кассы такого товара — вид 'length'.

-- Ограничения ищутся по определению, а не по имени: на разных базах имена могут различаться.
do $$
declare
    r record;
begin
    for r in
        select conrelid::regclass::text as tbl, conname
        from pg_constraint
        where contype = 'c'
          and (
              (conrelid = 'public.products'::regclass
                  and pg_get_constraintdef(oid) ~ '\(unit = ANY')
              or (conrelid in ('public.sale_lines'::regclass, 'public.cancelled_lines'::regclass)
                  and pg_get_constraintdef(oid) ~ '\(kind = ANY')
              or (conrelid = 'public.product_unit_changes'::regclass
                  and pg_get_constraintdef(oid) ~ '\((from|to)_unit = ANY')
          )
    loop
        execute format('alter table %s drop constraint %I', r.tbl, r.conname);
    end loop;
end $$;

alter table products add constraint products_unit_check
    check (unit in ('piece', 'ml', 'g', 'mm'));
alter table sale_lines add constraint sale_lines_kind_check
    check (kind in ('piece', 'container', 'pour', 'service', 'weight', 'length'));
alter table cancelled_lines add constraint cancelled_lines_kind_check
    check (kind in ('piece', 'container', 'pour', 'weight', 'length', 'service'));
alter table product_unit_changes add constraint product_unit_changes_from_unit_check
    check (from_unit in ('piece', 'ml', 'g', 'mm'));
alter table product_unit_changes add constraint product_unit_changes_to_unit_check
    check (to_unit in ('piece', 'ml', 'g', 'mm'));

-- Смена между разными единицами без коэффициента (кг ↔ шт, кг ↔ м и т. п.): количества прошлых
-- документов в нынешние единицы не переводятся — возврат и сторно по ним отказывают (409).
alter table product_unit_changes add column reset boolean not null default false;

-- До 0096 с коэффициентом менялись только литры ↔ штуки и канистра; всё с 'g' — смены быстрой
-- правки (только без остатка). Журнал неизменяем, поэтому триггер на время выключается.
alter table product_unit_changes disable trigger product_unit_changes_immutable;
update product_unit_changes set reset = true
where from_unit <> to_unit
  and not (from_unit in ('ml', 'piece') and to_unit in ('ml', 'piece'));
alter table product_unit_changes enable trigger product_unit_changes_immutable;
