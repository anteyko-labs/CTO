-- Оплата сотрудников: правила, начисления, выплаты (SPEC-07, ADR-019, ADR-035, ADR-036).

create table employee_pay_rules (
    id           uuid primary key,
    branch_id    uuid not null references branches,
    employee_id  uuid not null references employees,
    kind         text not null check (kind in ('per_service', 'revenue_percent', 'per_shift', 'monthly_salary')),
    role         text not null default 'cashier' check (role in ('cashier', 'master')),
    base         text check (base in ('gross', 'sale_total', 'goods', 'services')),
    amount_tyiyn bigint check (amount_tyiyn >= 0),
    rate_bp      integer check (rate_bp >= 0),
    active_from  date not null default (now() at time zone 'Asia/Bishkek')::date,
    active_to    date,
    user_id      uuid not null references users,
    created_at   timestamptz not null default now()
);
create index employee_pay_rules_idx on employee_pay_rules (branch_id, employee_id, kind);

create table payroll_accruals (
    id            uuid primary key,
    branch_id     uuid not null references branches,
    employee_id   uuid not null references employees,
    business_date date not null,
    kind          text not null check (kind in ('service_fee', 'revenue_percent', 'shift_fee',
                                                'monthly_salary', 'bonus', 'penalty', 'shortage')),
    amount_tyiyn  bigint not null,
    base_tyiyn    bigint,
    rule_id       uuid references employee_pay_rules,
    doc_type      text not null default '',
    doc_id        uuid,
    comment       text not null default '',
    reversal_of   uuid references payroll_accruals,
    user_id       uuid not null references users,
    device_id     uuid not null,
    created_at    timestamptz not null default now()
);
create index payroll_accruals_idx on payroll_accruals (branch_id, business_date);
create index payroll_accruals_employee_idx on payroll_accruals (employee_id, business_date);

-- Процент с долговой части чека ждёт погашения (ADR-035).
create table payroll_pending (
    id                   uuid primary key,
    branch_id            uuid not null references branches,
    party_id             uuid not null references parties,
    sale_id              uuid not null references sales,
    employee_id          uuid not null references employees,
    rule_id              uuid references employee_pay_rules,
    rate_bp              integer not null,
    gross_tyiyn          bigint not null,
    debt_total_tyiyn     bigint not null check (debt_total_tyiyn > 0),
    debt_remaining_tyiyn bigint not null,
    created_at           timestamptz not null default now()
);
create index payroll_pending_party_idx on payroll_pending (party_id, created_at);

create table payouts (
    id           uuid primary key,
    branch_id    uuid not null references branches,
    number       bigint not null,
    employee_id  uuid not null references employees,
    amount_tyiyn bigint not null,
    source       text not null check (source in ('account', 'outside')),
    account_id   uuid references cash_accounts,
    comment      text not null default '',
    reversal_of  uuid references payouts,
    user_id      uuid not null references users,
    device_id    uuid not null,
    created_at   timestamptz not null default now(),
    unique (branch_id, number)
);
create index payouts_employee_idx on payouts (employee_id, created_at desc);

create trigger payroll_accruals_immutable before update or delete on payroll_accruals
    for each row execute function forbid_change();
create trigger payouts_immutable before update or delete on payouts
    for each row execute function forbid_change();

-- Начисления за уже проведённые замены: ставка зафиксирована в чеках.
insert into payroll_accruals (id, branch_id, employee_id, business_date, kind, amount_tyiyn,
                              doc_type, doc_id, user_id, device_id, created_at)
select gen_random_uuid(), s.branch_id, s.master_id, s.business_date, 'service_fee',
       s.master_fee_tyiyn, 'sale', s.id, s.user_id, s.device_id, s.created_at
from sales s
where s.master_id is not null and s.master_fee_tyiyn <> 0;
