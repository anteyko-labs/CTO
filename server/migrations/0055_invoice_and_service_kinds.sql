-- Доработки по проверке на точке, часть 4 (SPEC-30 §1–2, ADR-095, ADR-096).

-- 1. Оплата «По счёту» (ADR-095): юрлицо платит перечислением по выставленному счёту.
alter table sale_payments drop constraint sale_payments_method_check;
alter table sale_payments add constraint sale_payments_method_check
    check (method in ('cash', 'card', 'transfer', 'debt', 'bonus', 'battery', 'invoice'));

-- Безналичный счёт по умолчанию для «По счёту» — как для карты и QR (ADR-077). Пока не выбран,
-- деньги ложатся на счёт по умолчанию для карты.
alter table cash_accounts add column default_invoice boolean not null default false;
alter table cash_accounts drop constraint cash_accounts_default_bank_check;
alter table cash_accounts add constraint cash_accounts_default_bank_check
    check ((not default_card and not default_transfer and not default_invoice) or kind = 'bank');
create unique index cash_accounts_default_invoice on cash_accounts (branch_id) where default_invoice;

-- 2. Вид замены и ставка мастеру — по услуге (ADR-096). «Бесплатно с нашим маслом»: в чеке с
-- нашим маслом услуга стоит 0, клиент со своим маслом платит цену из справочника.
alter table services add column free_with_oil boolean not null default false;
update services set free_with_oil = true where active and name ilike '%масл%';
-- Мастеру за замену платила ставка за чек из настроек; услуга без своей ставки (0, например
-- загруженная из старой программы) получает её, чтобы новые чеки не оставили мастера без оплаты.
update services s
set master_fee_tyiyn = coalesce((select greatest((x.value ->> 'oil_change_master_fee_tyiyn')::bigint, 0)
                                 from settings x
                                 where x.branch_id = s.branch_id and x.key = 'sales'
                                   and jsonb_typeof(x.value -> 'oil_change_master_fee_tyiyn') = 'number'), 3000)
where s.free_with_oil and s.master_fee_tyiyn = 0;

-- В филиале без услуги замены масла заводятся две: цена 0 (масло наше), мастеру — прежняя ставка
-- за замену из настроек (по умолчанию 30 сом). Цены владелец правит сам.
insert into services (id, branch_id, name, price_tyiyn, master_fee_tyiyn, free_with_oil)
select gen_random_uuid(), b.id, x.name, 0,
       coalesce((select greatest((s.value ->> 'oil_change_master_fee_tyiyn')::bigint, 0)
                 from settings s
                 where s.branch_id = b.id and s.key = 'sales'
                   and jsonb_typeof(s.value -> 'oil_change_master_fee_tyiyn') = 'number'), 3000),
       true
from branches b
cross join (values (1, 'Замена масла в моторе'), (2, 'Замена масла в коробке')) as x(n, name)
where not exists (select 1 from services s
                  where s.branch_id = b.id and s.active and s.name ilike '%масл%')
order by b.id, x.n;
