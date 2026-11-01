-- Сверка базы до и после переноса (Мак → облако) и после восстановления копии.
-- Только читает. Запуск: psql -X -At -v ON_ERROR_STOP=1 -d avtodom -f verify.sql > итог.txt
-- Строки «склад», «касса», «долги», «продажи», «журнал» — суммы, они должны совпасть и после
-- того, как новая версия сервера применит свои миграции. Строки «строк» (число строк каждой
-- таблицы) и «миграции» совпадают только до первого запуска нового сервера.

-- Остаток товара по филиалам: сколько товаров с ненулевым остатком, количество (мл/шт) и сумма
-- себестоимости в тыйынах — сумма движений склада (I6).
select format('склад | %s | товаров с остатком %s | количество %s | себестоимость %s',
              b.id, count(*) filter (where s.q <> 0), coalesce(sum(s.q), 0), coalesce(sum(s.v), 0))
from branches b
left join (select branch_id, product_id, sum(qty_delta) q, sum(value_delta_tyiyn) v
           from stock_movements group by branch_id, product_id) s on s.branch_id = b.id
group by b.id
order by b.id;

-- Кассы и счета: остаток счёта и сумма его движений.
select format('касса | %s | %s | %s | остаток %s | по движениям %s | движений %s',
              a.branch_id, a.name, a.kind, a.balance_tyiyn, coalesce(sum(m.amount_tyiyn), 0), count(m.id))
from cash_accounts a
left join cash_movements m on m.account_id = a.id
group by a.id
order by a.branch_id, a.name collate "C", a.id;

-- Долги: клиенты (> 0 — должны нам) и поставщики (< 0 — должны мы) по филиалам.
select format('долги | %s | %s | плюс %s | минус %s | ненулевых %s',
              pb.branch_id, p.role,
              coalesce(sum(pb.balance_tyiyn) filter (where pb.balance_tyiyn > 0), 0),
              coalesce(sum(pb.balance_tyiyn) filter (where pb.balance_tyiyn < 0), 0),
              count(*) filter (where pb.balance_tyiyn <> 0))
from party_balances pb
join parties p on p.id = pb.party_id
group by pb.branch_id, p.role
order by pb.branch_id, p.role;

select format('долги | записи | %s | %s | %s | сумма %s', branch_id, kind, count(*), sum(amount_tyiyn))
from party_ledger
group by branch_id, kind
order by branch_id, kind;

-- Чеки по филиалам: число, сумма, последний номер и время последнего (UTC).
select format('продажи | %s | чеков %s | сумма %s | последний № %s | %s',
              branch_id, count(*), sum(total_tyiyn), max(number),
              to_char(max(created_at) at time zone 'UTC', 'YYYY-MM-DD HH24:MI:SS'))
from sales
group by branch_id
order by branch_id;

select format('журнал | записей %s | последняя %s', count(*),
              to_char(max(created_at) at time zone 'UTC', 'YYYY-MM-DD HH24:MI:SS'))
from audit_log;

select format('миграции | %s | последняя %s', count(*), max(version)) from _sqlx_migrations;

-- Число строк каждой таблицы.
select format('select %L || '' | '' || count(*) from public.%I', 'строк | ' || table_name, table_name)
from information_schema.tables
where table_schema = 'public' and table_type = 'BASE TABLE'
order by table_name
\gexec
