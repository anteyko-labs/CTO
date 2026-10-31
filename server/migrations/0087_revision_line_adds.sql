-- Кто сколько посчитал в строке ревизии (задача 003, Н14): один товар лежит в нескольких местах,
-- его считают разные люди с разных устройств — строка хранит общий итог, а здесь — каждая
-- прибавка («+»), каждый записанный итог («=») и «убрал строку». Журнал только пополняется (I5).
create table revision_line_adds (
    id          uuid primary key,
    revision_id uuid not null references revisions,
    product_id  uuid not null references products,
    mode        text not null check (mode in ('add', 'set', 'clear')),
    qty         bigint not null check (qty >= 0),
    user_id     uuid not null references users,
    device_id   uuid not null,
    op_id       uuid,
    created_at  timestamptz not null default now()
);
create index revision_line_adds_line on revision_line_adds (revision_id, product_id, created_at);
create trigger revision_line_adds_append_only before update or delete on revision_line_adds
    for each row execute function forbid_change();
