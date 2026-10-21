-- Отменённые товары (SPEC-30 §8, ADR-099): строки, удалённые из чека до проведения, —
-- крестиком, «Очистить» или вместе с отложенным чеком. Только добавляются, как журнал.
create table cancelled_lines (
    id                  uuid primary key,
    branch_id           uuid not null references branches,
    -- Операция, которой строки пришли: проведённый чек или отдельная запись (`op_id` с кассы).
    op_id               uuid not null,
    -- Чек, проведённый после удаления строки; у «Очистить» и отложенного — пусто.
    sale_id             uuid references sales,
    shift_id            uuid references shifts,
    cashier_employee_id uuid references employees,
    -- `remove` — крестиком, `clear` — «Очистить», `held` — удалили отложенный чек.
    reason              text not null check (reason in ('remove', 'clear', 'held')),
    kind                text not null check (kind in ('piece', 'container', 'pour', 'weight', 'service')),
    product_id          uuid references products,
    service_id          uuid references services,
    -- Название на момент удаления: карточку могут переименовать.
    name                text not null,
    qty                 bigint not null check (qty > 0),
    unit_price_tyiyn    bigint not null check (unit_price_tyiyn >= 0),
    amount_tyiyn        bigint not null check (amount_tyiyn >= 0),
    user_id             uuid not null references users,
    device_id           uuid not null,
    -- Когда удалили на устройстве (без сети строки приходят позже).
    removed_at          timestamptz not null,
    created_at          timestamptz not null default now()
);
create index cancelled_lines_shift_idx on cancelled_lines (shift_id, removed_at);
create index cancelled_lines_branch_time_idx on cancelled_lines (branch_id, removed_at);
create index cancelled_lines_sale_idx on cancelled_lines (sale_id) where sale_id is not null;

create trigger cancelled_lines_immutable before update or delete on cancelled_lines
    for each row execute function forbid_change();
