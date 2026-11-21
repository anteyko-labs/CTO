-- Объединение карточек клиентов и поставщиков (задача 043).
-- Влитая карточка уходит в архив и помнит, куда её влили; её чеки остаются за ней (I4),
-- а карточка, в которую влили, показывает их вместе со своими.
alter table parties add column merged_into uuid references parties;
create index parties_merged_into_idx on parties (merged_into) where merged_into is not null;

-- Баллы влитой карточки переходят двумя записями, как долг.
alter table loyalty_ledger drop constraint loyalty_ledger_kind_check;
alter table loyalty_ledger add constraint loyalty_ledger_kind_check
    check (kind in ('accrual', 'redeem', 'refund', 'accrual_back', 'merge'));
