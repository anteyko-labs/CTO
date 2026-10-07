-- Контрагенты, их работники и машины, долги (SPEC-10, ADR-030).

create table parties (
    id                 uuid primary key,
    branch_id          uuid not null references branches,
    role               text not null check (role in ('customer', 'supplier')),
    kind               text not null check (kind in ('person', 'company')),
    name               text not null,
    phone              text not null default '',
    inn                text not null default '',
    comment            text not null default '',
    credit_limit_tyiyn bigint check (credit_limit_tyiyn >= 0),
    due_days           integer check (due_days >= 0),
    active             boolean not null default true,
    -- Кэш: > 0 должны нам, < 0 аванс клиента или наш долг поставщику.
    balance_tyiyn      bigint not null default 0,
    created_at         timestamptz not null default now()
);
create index parties_branch_role_idx on parties (branch_id, role, active);
create index parties_name_trgm_idx on parties using gin (name gin_trgm_ops);

-- Работники фирмы: кто приезжает за товаром.
create table party_contacts (
    id         uuid primary key,
    branch_id  uuid not null references branches,
    party_id   uuid not null references parties,
    full_name  text not null,
    phone      text not null default '',
    position   text not null default '',
    inn        text not null default '',
    active     boolean not null default true,
    created_at timestamptz not null default now()
);
create index party_contacts_party_idx on party_contacts (party_id);

create table party_vehicles (
    id         uuid primary key,
    branch_id  uuid not null references branches,
    party_id   uuid not null references parties,
    plate      text not null,
    brand      text not null default '',
    model      text not null default '',
    comment    text not null default '',
    active     boolean not null default true,
    created_at timestamptz not null default now()
);
create index party_vehicles_party_idx on party_vehicles (party_id);

create table party_ledger (
    id            uuid primary key,
    branch_id     uuid not null references branches,
    party_id      uuid not null references parties,
    kind          text not null check (kind in ('debt', 'repayment', 'adjust')),
    amount_tyiyn  bigint not null,
    business_date date not null,
    doc_type      text not null default '',
    doc_id        uuid,
    comment       text not null default '',
    reversal_of   uuid references party_ledger,
    user_id       uuid not null references users,
    device_id     uuid not null,
    created_at    timestamptz not null default now()
);
create index party_ledger_party_idx on party_ledger (party_id, created_at desc);
create trigger party_ledger_immutable before update or delete on party_ledger
    for each row execute function forbid_change();

-- Поставщики этапа 1 переезжают в общий справочник; старая таблица остаётся за приходом.
insert into parties (id, branch_id, role, kind, name, phone, comment, active)
select s.id, b.id, 'supplier', 'company', s.name, s.phone, s.comment, s.active
from suppliers s cross join (select id from branches order by created_at limit 1) b;

-- Чек знает, кто покупал, кто приехал и на какой машине.
alter table sales add column party_id uuid references parties;
alter table sales add column contact_id uuid references party_contacts;
alter table sales add column vehicle_id uuid references party_vehicles;
create index sales_party_idx on sales (party_id, created_at desc);

-- Оплата «в долг» — такой же способ оплаты, как наличные и карта.
alter table sale_payments drop constraint sale_payments_method_check;
alter table sale_payments add constraint sale_payments_method_check
    check (method in ('cash', 'card', 'transfer', 'debt'));
