-- Временный разбор остатков из Умага (задача 042): количество строки выгрузки и итог разбора.
-- `intake_revision_id` и `intake_counted` — черновик ревизии и сколько в него записано (в
-- единицах товара), чтобы «Отменить» убрало ровно это.
alter table old_catalog
    add column qty numeric,
    add column intake_status text not null default 'todo'
        check (intake_status in ('todo', 'new', 'linked', 'skipped')),
    add column intake_product_id uuid references products,
    add column intake_revision_id uuid references revisions,
    add column intake_counted bigint;
create index old_catalog_intake_idx on old_catalog (branch_id, intake_status);
