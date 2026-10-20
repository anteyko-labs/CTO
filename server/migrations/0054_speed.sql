-- Скорость на большой истории (ADR-094): индексы под частые запросы и распределение оплат
-- поставщику без повторного просмотра всей истории на каждом шаге. Данные не меняются.

-- Деньги документа: накладные, сверка, исправления, бот ищут движения кассы по документу.
create index if not exists cash_movements_doc_idx on cash_movements (doc_id) where doc_id is not null;

-- Остаток кассы на начало периода — сумма движений до даты без чтения самих строк.
create index if not exists cash_movements_account_cover_idx
    on cash_movements (account_id, created_at) include (amount_tyiyn);
drop index if exists cash_movements_account_idx;

-- Комиссия банка с погашений долга — по учётному дню (отчёт о прибыли, сводка).
create index if not exists cash_movements_repay_fee_day_idx
    on cash_movements (branch_id, ((created_at at time zone 'Asia/Bishkek')::date))
    where kind = 'bank_fee' and doc_type = 'repayment';

-- Продажи товара: «Куда сколько поступило» (что из накладной продано и почём).
create index if not exists sale_lines_product_idx on sale_lines (product_id);

-- Переоценка и ревизия по дням — себестоимость в отчёте о прибыли.
create index if not exists stock_movements_reval_day_idx
    on stock_movements (branch_id, ((created_at at time zone 'Asia/Bishkek')::date))
    where doc_type in ('revaluation', 'revision');

-- Последняя продажа и первый приход товара — залежалый товар на каждой сводке.
create index if not exists stock_movements_product_doc_idx
    on stock_movements (branch_id, product_id, doc_type, created_at);
drop index if exists stock_movements_product_idx;

-- Долг по оплате труда на сводке: начислено по работнику.
create index if not exists payroll_accruals_employee_amount_idx
    on payroll_accruals (employee_id) include (amount_tyiyn);

-- Распределение долга поставщика (SPEC-27 §5, ADR-073) — то же правило, что в 0040:
-- снятые записи теряют свои распределения, затем открытые оплаты ложатся на самые старые
-- открытые долги (по учётному дню, затем по времени записи, затем по id). Открытые долги и
-- оплаты читаются один раз и сводятся по порядку, а не ищутся заново на каждом шаге; если
-- открытых оплат нет, долги не читаются вовсе.
create or replace function supplier_allocate(p_party uuid) returns integer language plpgsql as $$
declare
    k       integer;
    n       integer := 0;
    d_ids   uuid[];
    d_br    uuid[];
    d_open  bigint[];
    c_ids   uuid[];
    c_open  bigint[];
    i       integer := 1;
    j       integer := 1;
    take    bigint;
begin
    insert into supplier_allocations (id, branch_id, party_id, credit_id, debit_id, amount_tyiyn, reversal_of)
    select gen_random_uuid(), s.branch_id, s.party_id, s.credit_id, s.debit_id, -s.amount_tyiyn, s.id
    from supplier_allocations s
    join party_ledger cr on cr.id = s.credit_id
    join party_ledger db on db.id = s.debit_id
    where s.party_id = p_party and s.reversal_of is null
      and not exists (select 1 from supplier_allocations y where y.reversal_of = s.id)
      and not (supplier_ledger_live(cr) and supplier_ledger_live(db))
    order by s.created_at, s.id;
    get diagnostics k = row_count;
    n := n + k;

    select array_agg(x.id order by x.business_date, x.created_at, x.id),
           array_agg(x.open order by x.business_date, x.created_at, x.id)
      into c_ids, c_open
    from (select l.id, l.business_date, l.created_at,
                 (l.amount_tyiyn - coalesce((select sum(s.amount_tyiyn) from supplier_allocations s
                                             where s.credit_id = l.id), 0))::bigint as open
          from party_ledger l
          where l.party_id = p_party and l.amount_tyiyn > 0 and supplier_ledger_live(l)) x
    where x.open > 0;
    if c_ids is null then
        return n;
    end if;

    select array_agg(x.id order by x.business_date, x.created_at, x.id),
           array_agg(x.branch_id order by x.business_date, x.created_at, x.id),
           array_agg(x.open order by x.business_date, x.created_at, x.id)
      into d_ids, d_br, d_open
    from (select l.id, l.branch_id, l.business_date, l.created_at,
                 (-l.amount_tyiyn - coalesce((select sum(s.amount_tyiyn) from supplier_allocations s
                                              where s.debit_id = l.id), 0))::bigint as open
          from party_ledger l
          where l.party_id = p_party and l.amount_tyiyn < 0 and supplier_ledger_live(l)) x
    where x.open > 0;
    if d_ids is null then
        return n;
    end if;

    while i <= array_length(d_ids, 1) and j <= array_length(c_ids, 1) loop
        take := least(d_open[i], c_open[j]);
        insert into supplier_allocations (id, branch_id, party_id, credit_id, debit_id, amount_tyiyn)
        values (gen_random_uuid(), d_br[i], p_party, c_ids[j], d_ids[i], take);
        n := n + 1;
        d_open[i] := d_open[i] - take;
        c_open[j] := c_open[j] - take;
        if d_open[i] = 0 then
            i := i + 1;
        end if;
        if c_open[j] = 0 then
            j := j + 1;
        end if;
    end loop;
    return n;
end
$$;
