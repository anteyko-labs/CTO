-- Оплата накладных поставщику по частям (SPEC-27 §5, ADR-073): что чем закрыто.
--
-- Долги перед поставщиком — записи `party_ledger` с минусом: накладная в долг (`receipt`) и правка
-- владельцем в сторону долга (`adjust`, старый долг из тетради). Закрывают их записи с плюсом:
-- оплата (`repayment`), зачёт брака скидкой (`defect`), правка в нашу пользу (`adjust`).
-- Распределение — отдельные неизменяемые строки «закрывающая запись ↔ долг ↔ сумма»; отменяются
-- только обратной строкой со ссылкой на исходную.

-- Запись участвует в распределении, пока её не сняли: не сторно и не сторнирована, не сторно
-- накладной и не снятая им накладная, не отмена решения по браку и не отменённое решение.
-- Каждая такая пара в сумме даёт ноль, поэтому баланс = открытые оплаты − открытые долги.
create function supplier_ledger_live(e party_ledger) returns boolean language sql stable as $$
    select e.amount_tyiyn <> 0
       and e.reversal_of is null
       and not exists (select 1 from party_ledger x where x.reversal_of = e.id)
       and e.doc_type not in ('receipt_reversal', 'defect_reversal')
       and not (e.doc_type = 'receipt'
                and exists (select 1 from receipts r where r.reversal_of = e.doc_id))
       and not (e.doc_type = 'defect'
                and exists (select 1 from defect_resolutions d where d.reversal_of = e.doc_id))
$$;

create table supplier_allocations (
    id           uuid primary key,
    branch_id    uuid not null references branches,
    party_id     uuid not null references parties,
    -- Чем закрыто: оплата, зачёт брака или правка в нашу пользу.
    credit_id    uuid not null references party_ledger,
    -- Что закрыто: накладная в долг или правка долга.
    debit_id     uuid not null references party_ledger,
    amount_tyiyn bigint not null,
    -- Отмена распределения (сторно оплаты, накладной, решения по браку) — обратная строка.
    reversal_of  uuid unique references supplier_allocations,
    created_at   timestamptz not null default now(),
    check ((reversal_of is null) = (amount_tyiyn > 0))
);
create index supplier_allocations_debit_idx on supplier_allocations (debit_id);
create index supplier_allocations_credit_idx on supplier_allocations (credit_id);
create index supplier_allocations_party_idx on supplier_allocations (party_id, created_at);
create trigger supplier_allocations_immutable before update or delete on supplier_allocations
    for each row execute function forbid_change();

-- Распределение долга поставщика: снятые записи теряют свои распределения, затем открытые
-- оплаты ложатся на самые старые открытые долги (по учётному дню, затем по времени записи).
-- Вызывается в транзакции, которая записала движение долга, под блокировкой контрагента.
-- Возвращает число новых строк.
create function supplier_allocate(p_party uuid) returns integer language plpgsql as $$
declare
    a      record;
    d      record;
    c      record;
    take   bigint;
    n      integer := 0;
begin
    for a in
        select s.id, s.branch_id, s.party_id, s.credit_id, s.debit_id, s.amount_tyiyn
        from supplier_allocations s
        join party_ledger cr on cr.id = s.credit_id
        join party_ledger db on db.id = s.debit_id
        where s.party_id = p_party and s.reversal_of is null
          and not exists (select 1 from supplier_allocations y where y.reversal_of = s.id)
          and not (supplier_ledger_live(cr) and supplier_ledger_live(db))
        order by s.created_at, s.id
    loop
        insert into supplier_allocations (id, branch_id, party_id, credit_id, debit_id, amount_tyiyn, reversal_of)
        values (gen_random_uuid(), a.branch_id, a.party_id, a.credit_id, a.debit_id, -a.amount_tyiyn, a.id);
        n := n + 1;
    end loop;
    loop
        select l.id, l.branch_id,
               -l.amount_tyiyn - coalesce((select sum(s.amount_tyiyn) from supplier_allocations s
                                           where s.debit_id = l.id), 0) as open
          into d
        from party_ledger l
        where l.party_id = p_party and l.amount_tyiyn < 0 and supplier_ledger_live(l)
          and -l.amount_tyiyn > coalesce((select sum(s.amount_tyiyn) from supplier_allocations s
                                          where s.debit_id = l.id), 0)
        order by l.business_date, l.created_at, l.id
        limit 1;
        exit when not found;
        select l.id,
               l.amount_tyiyn - coalesce((select sum(s.amount_tyiyn) from supplier_allocations s
                                          where s.credit_id = l.id), 0) as open
          into c
        from party_ledger l
        where l.party_id = p_party and l.amount_tyiyn > 0 and supplier_ledger_live(l)
          and l.amount_tyiyn > coalesce((select sum(s.amount_tyiyn) from supplier_allocations s
                                         where s.credit_id = l.id), 0)
        order by l.business_date, l.created_at, l.id
        limit 1;
        exit when not found;
        take := least(d.open, c.open);
        insert into supplier_allocations (id, branch_id, party_id, credit_id, debit_id, amount_tyiyn)
        values (gen_random_uuid(), d.branch_id, p_party, c.id, d.id, take);
        n := n + 1;
    end loop;
    return n;
end
$$;

-- Оплаты, сделанные до этой миграции, распределяются задним числом на самые старые накладные:
-- статусы накладных сходятся с текущим долгом поставщика.
select supplier_allocate(p.id) from parties p where p.role = 'supplier';
