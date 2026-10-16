-- Напоминания клиенту о сроке долга в боте (SPEC-27 §7): за 3 дня и в день срока, каждое один раз.
-- Срок — последний день оплаты самого старого непогашенного долга (SPEC-10); сдвинулся срок
-- (долг погашен частично или взят новый) — это другое напоминание.
create table debt_reminders (
    party_id    uuid not null references parties,
    due_date    date not null,
    days_before integer not null check (days_before in (0, 3)),
    sent_at     timestamptz not null default now(),
    primary key (party_id, due_date, days_before)
);
