-- Несколько филиалов (SPEC-33, ADR-108), продолжение 0062.

-- 1. Долг поставщику по филиалам: распределение оплат (ADR-073) — внутри филиала. Прежние
-- распределения — в одном филиале и остаются как есть.
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
    br      uuid;
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

    -- Долг поставщику у каждого филиала свой (SPEC-33): оплата филиала закрывает только его
    -- накладные.
    for br in select distinct l.branch_id from party_ledger l where l.party_id = p_party loop
        select array_agg(x.id order by x.business_date, x.created_at, x.id),
               array_agg(x.open order by x.business_date, x.created_at, x.id)
          into c_ids, c_open
        from (select l.id, l.business_date, l.created_at,
                     (l.amount_tyiyn - coalesce((select sum(s.amount_tyiyn) from supplier_allocations s
                                                 where s.credit_id = l.id), 0))::bigint as open
              from party_ledger l
              where l.party_id = p_party and l.branch_id = br
                and l.amount_tyiyn > 0 and supplier_ledger_live(l)) x
        where x.open > 0;
        if c_ids is null then
            continue;
        end if;

        select array_agg(x.id order by x.business_date, x.created_at, x.id),
               array_agg(x.branch_id order by x.business_date, x.created_at, x.id),
               array_agg(x.open order by x.business_date, x.created_at, x.id)
          into d_ids, d_br, d_open
        from (select l.id, l.branch_id, l.business_date, l.created_at,
                     (-l.amount_tyiyn - coalesce((select sum(s.amount_tyiyn) from supplier_allocations s
                                                  where s.debit_id = l.id), 0))::bigint as open
              from party_ledger l
              where l.party_id = p_party and l.branch_id = br
                and l.amount_tyiyn < 0 and supplier_ledger_live(l)) x
        where x.open > 0;
        if d_ids is null then
            continue;
        end if;

        i := 1;
        j := 1;
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
    end loop;
    return n;
end
$$;

-- 2. Строка филиала без цены (заведённая лениво, первым движением) берёт общую; цену,
-- поставленную явно при заведении строки, триггер не трогает.
create or replace function branch_products_shared_price() returns trigger language plpgsql as $$
begin
    if not new.own_price and new.sale_price_tyiyn = 0 then
        new.sale_price_tyiyn := coalesce(
            (select p.sale_price_tyiyn from products p where p.id = new.product_id), 0);
    end if;
    return new;
end $$;

-- 3. Бот владельца (SPEC-33 §3): `telegram_links.branch_id` — филиал, выбранный в чате (отчёты,
-- черновики, вопросы); «Все филиалы» — только для отчётов.
alter table telegram_links add column all_branches boolean not null default false;
