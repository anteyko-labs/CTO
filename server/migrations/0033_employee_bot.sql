-- Кабинет работника в Телеграм-боте (SPEC-23, ADR-066).

-- Телефон в карточке сотрудника: задаёт владелец, по последним 9 цифрам работник подключается к боту.
alter table employees add column phone text not null default '';

-- Чат работника: один чат на сотрудника. `phone` — номер карточки на момент подключения:
-- сменился в карточке — чат отвязывается.
create table telegram_employees (
    chat_id     bigint primary key,
    employee_id uuid not null unique references employees,
    branch_id   uuid not null references branches,
    phone       text not null,
    linked_at   timestamptz not null default now()
);

-- Действия чата работника за последнюю минуту: не больше 30 (просмотры в журнал не пишутся).
-- Служебный счётчик чата, а не учёт: без branch_id, как telegram_code_failures (ADR-063);
-- записи старше минуты удаляются.
create table telegram_hits (
    chat_id bigint not null,
    at      timestamptz not null default now()
);
create index telegram_hits_chat_idx on telegram_hits (chat_id, at);
create index telegram_hits_at_idx on telegram_hits (at);
