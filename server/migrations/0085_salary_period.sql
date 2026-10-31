-- Оклад (задача 003 Н11): дата выплаты в расчёте и расчётный день оклада у работника
-- (ADR-{B9.1}, ADR-{B9.2}).

-- Дата выплаты в расчёте: в какой день, месяц и период оклада выплата видна. Деньги уходят в
-- момент проведения (движение кассы — сейчас, в открытой смене); дата влияет только на расчёт.
-- Прежние выплаты получают свой день проведения по Бишкеку — расчёт по ним не меняется. Разовое
-- заполнение нового поля: защита от правки снимается только внутри миграции (как 0044).
alter table payouts add column pay_date date;
alter table payouts disable trigger payouts_immutable;
update payouts set pay_date = (created_at at time zone 'Asia/Bishkek')::date;
alter table payouts enable trigger payouts_immutable;
alter table payouts alter column pay_date set default ((now() at time zone 'Asia/Bishkek')::date);
alter table payouts alter column pay_date set not null;
create index payouts_pay_date_idx on payouts (employee_id, pay_date);

-- День расчёта оклада: период с D-го по (D−1)-е следующего месяца, к выплате D-го. По умолчанию
-- 1 — календарный месяц, как было. 1–28: такой день есть в каждом месяце.
alter table employees
    add column salary_day smallint not null default 1 check (salary_day between 1 and 28);

-- Период, за который начислен оклад. У прежних начислений пусто: их период — календарный месяц
-- дня начисления (так их и начисляли). Колонки без значения по умолчанию строки не переписывают.
alter table payroll_accruals
    add column period_from date,
    add column period_to date,
    add constraint payroll_accruals_period_check
        check (period_from is null or period_to is null or period_from <= period_to);
