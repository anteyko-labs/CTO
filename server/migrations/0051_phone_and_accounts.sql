-- Проверка 20.10.2026 (ADR-090).

-- Номер телефона для сравнения: кыргызский номер в любой записи — «996XXXXXXXXX», остальные —
-- всеми цифрами. То же правило — domain::phone::key.
create function phone_norm(p text) returns text language sql immutable as $$
  select case
           when d ~ '^996[0-9]{9}$' then d
           when d ~ '^0[0-9]{9}$' then '996' || substr(d, 2)
           when d ~ '^[0-9]{9}$' then '996' || d
           else d
         end
  from (select regexp_replace(coalesce(p, ''), '[^0-9]', '', 'g') as d) x
$$;

-- Срок оплаты — не больше 10 лет (дальше даты не помещаются в календарь).
update parties set due_days = 3650 where due_days > 3650;

-- Привязки Телеграма к группам (до запрета групп, ADR-070) лентой владельца не пользуются.
delete from telegram_links where chat_id < 0;

-- QR владельцу — на его личный счёт без комиссии (решение заказчика 17.10.2026): если карта
-- и перевод идут на один счёт, заводится «Личный счёт владельца» (0 %) и становится счётом
-- для перевода. Где счёт для перевода уже отдельный — ничего не меняется.
insert into cash_accounts (id, branch_id, name, kind)
select gen_random_uuid(), a.branch_id, 'Личный счёт владельца', 'bank'
from cash_accounts a
where a.default_card and a.default_transfer
  and not exists (select 1 from cash_accounts x
                  where x.branch_id = a.branch_id and x.name = 'Личный счёт владельца');
update cash_accounts a set default_transfer = false
where a.default_card and a.default_transfer
  and exists (select 1 from cash_accounts x
              where x.branch_id = a.branch_id and x.name = 'Личный счёт владельца' and x.kind = 'bank'
                and x.active and not x.owner_only);
update cash_accounts x set default_transfer = true
where x.name = 'Личный счёт владельца' and x.kind = 'bank' and x.active and not x.owner_only
  and not exists (select 1 from cash_accounts a where a.branch_id = x.branch_id and a.default_transfer);
