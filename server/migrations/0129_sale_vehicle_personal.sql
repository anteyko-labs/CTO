-- Задача 051, ADR-215: долг юрлица по видам — «машины фирмы и прочее» и «личные машины
-- работников» (машина с paid_by_party_id = фирма, ADR-199). Вид фиксируется в чеке при
-- проведении: перевод машины потом из личной в фирменную вид старого чека не меняет.
alter table sales add column vehicle_personal boolean not null default false;

-- Старые чеки — по текущей машине. Чек неизменяем (I4): новое поле заполняется один раз, при
-- выключенной защите, как в 0006 (на рабочей базе без этого миграция падает).
alter table sales disable trigger sales_immutable;
update sales s set vehicle_personal = true
from party_vehicles v
where v.id = s.vehicle_id and s.party_id is not null
  and v.paid_by_party_id = s.party_id and v.party_id <> s.party_id;
alter table sales enable trigger sales_immutable;

-- Остаток долга за личные машины работников: долги (чеки в долг и правки в плюс, за вычетом
-- возвратов и сторно — как у срока долга) от новых к старым, баланс гасит сначала старые
-- (ADR-073), поэтому неоплаченное — самые новые долги на сумму баланса (domain::debt).
-- Влитые карточки (задача 043): их долги — у основной, пары записей объединения не считаются.
-- p_branch — только долги филиала (null — все), p_upto — по этот день (null — все).
create function debt_personal_tyiyn(p_party uuid, p_balance bigint, p_branch uuid, p_upto date)
returns bigint language sql stable as $$
  with recursive g(id) as (
    select p_party
    union select x.id from parties x join g on x.merged_into = g.id
      where (select merged_into from parties where id = p_party) is null
  ),
  d as (
    select l.business_date, l.created_at, coalesce(s.vehicle_personal, false) as personal,
           (l.amount_tyiyn
             - coalesce((select sum(-x.amount_tyiyn) from party_ledger x join sales r on r.id = x.doc_id
                         where l.doc_type = 'sale' and x.doc_type = 'sale_return'
                           and r.reversal_of = l.doc_id and x.party_id = l.party_id
                           and (p_upto is null or x.business_date <= p_upto)), 0)
             - coalesce((select sum(-x.amount_tyiyn) from party_ledger x where x.reversal_of = l.id
                           and (p_upto is null or x.business_date <= p_upto)), 0)) as amt
    from party_ledger l
    left join sales s on l.doc_type = 'sale' and s.id = l.doc_id
    where l.party_id in (select id from g) and l.kind in ('debt', 'adjust') and l.amount_tyiyn > 0
      and (p_branch is null or l.branch_id = p_branch)
      and (p_upto is null or l.business_date <= p_upto)
      and not ((select count(*) from g) > 1 and l.doc_type = 'merge')
  ),
  w as (
    select amt, personal,
           sum(amt) over (order by business_date desc, created_at desc
                          rows between unbounded preceding and current row) as cum
    from d where amt > 0
  )
  select coalesce(sum(least(amt, greatest(0, p_balance - (cum - amt))))
                  filter (where personal), 0)::bigint
  from w
$$;
