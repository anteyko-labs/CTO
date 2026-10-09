-- Сторно начисления и выплаты сотруднику (SPEC-07): каждое — один раз.
create unique index payroll_accruals_reversal_once on payroll_accruals (reversal_of) where reversal_of is not null;
create unique index payouts_reversal_once on payouts (reversal_of) where reversal_of is not null;
