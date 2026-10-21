-- Права администраторов выбирает владелец (SPEC-30 §5, ADR-097). Список прав — `auth::PERMISSIONS`.
alter table user_permissions drop constraint user_permissions_permission_check;
alter table user_permissions add constraint user_permissions_permission_check check (permission in (
    'sale_return', 'sale_debt', 'discount', 'debt_payment', 'cash_moves', 'expenses',
    'payroll_pay', 'receipts', 'stock_in', 'supplier_returns', 'defects', 'revision',
    'oil_transfer', 'battery_intake', 'products', 'clients'
));

-- Права, выданные этим обновлением, никто не выдавал вручную: `granted_by` пустой.
alter table user_permissions alter column granted_by drop not null;

-- Существующим администраторам — все права со значением «да» по умолчанию: для них ничего не
-- меняется, кроме скидки (её теперь ставит владелец или администратор с правом). Перелив
-- остаётся у тех, кому владелец его уже выдал (ADR-082).
insert into user_permissions (user_id, branch_id, permission, granted_by)
select u.id, u.branch_id, p.permission, null
from users u
cross join unnest(array[
    'sale_return', 'sale_debt', 'debt_payment', 'cash_moves', 'expenses', 'payroll_pay',
    'receipts', 'stock_in', 'supplier_returns', 'defects', 'revision', 'battery_intake',
    'products', 'clients'
]) as p(permission)
where u.role = 'admin'
on conflict do nothing;
