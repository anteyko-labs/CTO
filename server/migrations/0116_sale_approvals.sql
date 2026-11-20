-- Подтверждение заправки юрлицом (задача 039 C, ADR-202): чек в долг фирмы — запрос фирме в
-- Телеграм и кабинет. Режим — настройка филиала `legal` (`settings`), здесь только запросы.
create table sale_approvals (
    id           uuid primary key,
    branch_id    uuid not null references branches,
    party_id     uuid not null references parties,
    -- op_id чека: по нему касса после подтверждения проводит тот же чек (I7, I8).
    op_id        uuid not null unique,
    sale_id      uuid references sales,
    -- wait — ждать, after — чек сразу, timeout — ждать N минут.
    mode         text not null check (mode in ('wait', 'after', 'timeout')),
    status       text not null check (status in ('pending', 'approved', 'rejected', 'expired', 'skipped')),
    -- Черновик чека (тело POST /sales) для режимов wait и timeout.
    draft        jsonb,
    -- Что видит фирма: машина, работник, позиции, сумма, кассир.
    summary      jsonb not null default '{}',
    debt_tyiyn   bigint not null,
    user_id      uuid not null references users,
    created_at   timestamptz not null default now(),
    expires_at   timestamptz,
    decided_at   timestamptz,
    decided_by   text not null default '',
    reason       text not null default '',
    -- «Провести без подтверждения» после таймаута.
    forced_at    timestamptz,
    forced_by    uuid references users
);
create index sale_approvals_party_idx on sale_approvals (party_id, created_at desc);
create unique index sale_approvals_sale_idx on sale_approvals (sale_id) where sale_id is not null;
create index sale_approvals_open_idx on sale_approvals (created_at) where status in ('pending', 'expired');

-- Какой чат фирмы уже получил запрос с кнопками.
create table sale_approval_notices (
    approval_id uuid not null references sale_approvals,
    chat_id     bigint not null,
    sent_at     timestamptz not null default now(),
    primary key (approval_id, chat_id)
);
