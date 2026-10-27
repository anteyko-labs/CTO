-- Выплаты по видам и аванс (SPEC-36 §11, ADR-122). У выплаты есть вид: за что выдали деньги.
-- Прежние выплаты — `general`: при чтении они раскладываются по видам по порядку
-- проценты → мастеру → бонусы → оклад, поэтому остатки и прибыль не меняются.
-- Колонка с постоянным значением по умолчанию не переписывает строки: запрет правки выплат
-- (триггер `payouts_immutable`) не срабатывает.
alter table payouts
    add column purpose text not null default 'general'
        check (purpose in ('general', 'percent', 'service_fee', 'bonus', 'salary', 'advance'));

-- Одна выплата по нескольким видам — одна операция: строки связаны её `op_id`.
alter table payouts add column op_id uuid;
create index payouts_op_idx on payouts (op_id) where op_id is not null;

-- Право администратора «Аванс мастерам» — по умолчанию не выдаётся никому.
alter table user_permissions drop constraint user_permissions_permission_check;
alter table user_permissions add constraint user_permissions_permission_check check (permission in (
    'sale_return', 'sale_debt', 'discount', 'debt_payment', 'cash_moves', 'expenses',
    'payroll_pay', 'payroll_advance', 'receipts', 'stock_in', 'supplier_returns', 'defects',
    'revision', 'oil_transfer', 'battery_intake', 'products', 'clients'
));
