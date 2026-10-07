-- Подарки: к товару привязан список того, что можно подарить (SPEC-11).

create table gift_rules (
    id                 uuid primary key,
    branch_id          uuid not null references branches,
    trigger_product_id uuid not null references products,
    active             boolean not null default true,
    user_id            uuid not null references users,
    created_at         timestamptz not null default now(),
    unique (branch_id, trigger_product_id)
);

create table gift_rule_items (
    rule_id         uuid not null references gift_rules on delete cascade,
    gift_product_id uuid not null references products,
    gift_qty        bigint not null check (gift_qty > 0),
    primary key (rule_id, gift_product_id)
);

-- Подарок — строка чека с нулевой ценой и признаком: это не скидка в 100 %.
alter table sale_lines add column gift boolean not null default false;
