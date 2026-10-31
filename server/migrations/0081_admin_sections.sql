-- Задача 003 Н8: права администратора — полный контроль владельца (ADR-151).
-- Группа «Разделы» (что администратор видит в меню и какие отчёты разделов отдаёт сервер),
-- акт сверки, сотрудники, услуги (по умолчанию «да» — как было), цены товаров и услуг, архив
-- товара, заработок кассиров (по умолчанию «нет» — как было).

-- 1. Список прав в проверке дополняется тем, что в ней уже есть: права, добавленные другими
--    обновлениями, не теряются при любом их порядке (как 0074).
do $$
declare
    def  text;
    keys text[];
    k    text;
begin
    select pg_get_constraintdef(oid) into def from pg_constraint
    where conrelid = 'public.user_permissions'::regclass
      and conname = 'user_permissions_permission_check';
    select coalesce(array_agg(t.m[1]), '{}') into keys
    from regexp_matches(coalesce(def, ''), '''([a-z_]+)''', 'g') as t(m);
    foreach k in array array[
        'view_sales', 'view_debts', 'view_payroll', 'view_income', 'view_accounts',
        'view_suppliers', 'view_stock', 'reconciliation', 'employees', 'services',
        'prices', 'products_archive', 'payroll_view_all'
    ] loop
        if not (k = any (keys)) then
            keys := keys || k;
        end if;
    end loop;
    alter table public.user_permissions drop constraint if exists user_permissions_permission_check;
    execute format(
        'alter table public.user_permissions add constraint user_permissions_permission_check check (permission in (%s))',
        (select string_agg(quote_literal(x), ', ') from unnest(keys) as x));
end
$$;

-- 2. Существующим администраторам — права «по умолчанию да»: сегодня они это уже видят и делают,
--    поэтому после обновления ничего не пропадает. «Цены», «Архив товара» и «Заработок кассиров»
--    не выдаются: раньше это было только у владельца.
insert into user_permissions (user_id, branch_id, permission, granted_by)
select u.id, u.branch_id, p, null
from users u
cross join unnest(array[
    'view_sales', 'view_debts', 'view_payroll', 'view_income', 'view_accounts',
    'view_suppliers', 'view_stock', 'reconciliation', 'employees', 'services'
]) as p
where u.role = 'admin'
on conflict do nothing;
