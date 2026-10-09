-- Оплата кассира по умолчанию (SPEC-07, ADR-019, ADR-036): 2 % с валовой прибыли чека
-- и оклад 30 000 сом. Заводится тем кассирам, у кого таких правил ещё нет; новым кассирам
-- сервер заводит их сам. Автор записи — первый владелец филиала.
insert into employee_pay_rules (id, branch_id, employee_id, kind, role, base, amount_tyiyn, rate_bp, user_id)
select gen_random_uuid(), e.branch_id, e.id, r.kind, 'cashier', r.base, r.amount, r.rate, o.id
from employees e
cross join (values ('revenue_percent', 'gross', null::bigint, 200),
                   ('monthly_salary', null, 3000000::bigint, null::integer)) as r(kind, base, amount, rate)
join lateral (select u.id from users u
              where u.branch_id = e.branch_id and u.role = 'owner'
              order by u.created_at limit 1) o on true
where e.is_cashier and e.active
  and not exists (select 1 from employee_pay_rules x
                  where x.employee_id = e.id and x.kind = r.kind and x.active_to is null);
