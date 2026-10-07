-- Операционные расходы по статьям (SPEC-06, ADR-023).

create table expense_articles (
    id         uuid primary key,
    branch_id  uuid not null references branches,
    name       text not null,
    owner_only boolean not null default false,
    active     boolean not null default true,
    created_at timestamptz not null default now(),
    unique (branch_id, name)
);

create table expenses (
    id           uuid primary key,
    branch_id    uuid not null references branches,
    number       bigint not null,
    article_id   uuid not null references expense_articles,
    amount_tyiyn bigint not null,
    source       text not null check (source in ('account', 'outside')),
    account_id   uuid references cash_accounts,
    expense_date date not null,
    comment      text not null default '',
    reversal_of  uuid references expenses,
    user_id      uuid not null references users,
    device_id    uuid not null,
    created_at   timestamptz not null default now(),
    unique (branch_id, number)
);
create index expenses_date_idx on expenses (branch_id, expense_date);

create trigger expenses_immutable before update or delete on expenses
    for each row execute function forbid_change();

do $$
declare b record;
begin
    for b in select id from branches loop
        insert into expense_articles (id, branch_id, name, owner_only) values
            (gen_random_uuid(), b.id, 'На развитие', false),
            (gen_random_uuid(), b.id, 'Хозтовары', false),
            (gen_random_uuid(), b.id, 'Налоги', false),
            (gen_random_uuid(), b.id, 'Личные расходы', true),
            (gen_random_uuid(), b.id, 'Прочее', false);
    end loop;
end $$;
