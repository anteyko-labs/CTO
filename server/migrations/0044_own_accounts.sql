-- Свои счета (SPEC-28 §5, ADR-076): владелец заводит и правит кассы и счета, как в «Обзоре
-- счетов» старой программы; у каждого — признак «отрицательный баланс разрешён»; у оплаты
-- картой и переводом — счёт, куда пришли деньги.

-- Можно ли уйти ниже нуля. Прежнее правило «в кассе столько нет» было общим для всех касс,
-- поэтому у заведённых касс и счетов — «не разрешён».
alter table cash_accounts add column allow_negative boolean not null default false;

-- Основной безналичный счёт: туда по умолчанию идут карта и перевод. Им становится прежний «Счёт».
update cash_accounts a set is_default = true
where a.kind = 'bank'
  and a.id = (select b.id from cash_accounts b
              where b.branch_id = a.branch_id and b.kind = 'bank'
              order by b.created_at, b.id limit 1);

-- Ровно одна основная касса смены и один основной безналичный счёт на филиал.
create unique index cash_accounts_default_register on cash_accounts (branch_id)
    where is_default and kind = 'register';
create unique index cash_accounts_default_bank on cash_accounts (branch_id)
    where is_default and kind = 'bank';

-- Счёт оплаты картой или переводом. Пусто — наличные, долг, баллы, аккумулятор.
alter table sale_payments add column account_id uuid references cash_accounts;

-- Прошлые оплаты картой и переводом пришли на единственный тогда «Счёт» филиала: записываем
-- его, чтобы возврат и отчёты по счетам знали, куда пришли деньги. Разовое заполнение нового
-- поля: защита от правки снимается только внутри миграции.
alter table sale_payments disable trigger sale_payments_immutable;
update sale_payments p
set account_id = (select a.id from cash_accounts a join sales s on s.branch_id = a.branch_id
                  where s.id = p.sale_id and a.kind = 'bank'
                  order by a.is_default desc, a.created_at limit 1)
where p.method in ('card', 'transfer');
alter table sale_payments enable trigger sale_payments_immutable;
create index sale_payments_account_idx on sale_payments (account_id) where account_id is not null;
