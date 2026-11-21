-- Задача 045: сотрудники общие для филиалов. Филиал без отметки «Свои сотрудники» видит
-- сотрудников всех таких же филиалов; с отметкой — только своих.
alter table branches add column own_staff boolean not null default false;

-- Филиалы, чьих сотрудников видит филиал b (включая сам b).
create function staff_branch_ids(b uuid) returns uuid[] language sql stable as $$
  select case when coalesce((select own_staff from branches where id = b), true)
              then array[b]
              else array(select id from branches where not own_staff) end
$$;
