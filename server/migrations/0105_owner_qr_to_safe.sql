-- Задача 029, ADR-189: «QR владельцу» ложится на «Сейф владельца», без комиссии банка.
alter table cash_accounts drop constraint cash_accounts_default_bank_check;
alter table cash_accounts add constraint cash_accounts_default_bank_check
    check (
        ((not default_card and not default_invoice) or kind = 'bank')
        and (not default_transfer or kind = 'bank' or (kind = 'safe' and owner_only))
    );

with safe as (
    select distinct on (branch_id) id, branch_id from cash_accounts
    where kind = 'safe' and owner_only and active
    order by branch_id, created_at
), cleared as (
    update cash_accounts a set default_transfer = false
    from safe s where a.branch_id = s.branch_id and a.default_transfer and a.id <> s.id
)
select 1;
update cash_accounts a set default_transfer = true
where a.id in (select distinct on (branch_id) id from cash_accounts
               where kind = 'safe' and owner_only and active
               order by branch_id, created_at);
