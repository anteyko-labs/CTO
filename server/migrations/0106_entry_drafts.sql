-- Незавершённые приход и оприходование (задача 030): поля формы как введены, не учётная запись.
create table entry_drafts (
    id uuid primary key,
    branch_id uuid not null references branches (id),
    user_id uuid not null references users (id),
    kind text not null check (kind in ('stock_in', 'receipt')),
    op_id uuid not null,
    seq bigint not null,
    body jsonb not null,
    updated_at timestamptz not null default now(),
    unique (branch_id, user_id, kind)
);
