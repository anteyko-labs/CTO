-- Ставка мастеру по умолчанию — 30 сом за каждую услугу (ADR-112, замечание заказчика
-- 26.10.2026: «по умолчанию за любое действие мастеру 30 сом»). Услугам со ставкой 0 (загружены
-- из старой программы или заведены без ставки) ставится ставка по умолчанию из настроек филиала
-- (`sales.oil_change_master_fee_tyiyn`), без настройки — 30 сом. Прежние чеки не меняются: ставка
-- зафиксирована в их строках (инвариант 3).
update services s
set master_fee_tyiyn = coalesce(
        (select (st.value ->> 'oil_change_master_fee_tyiyn')::bigint
         from settings st
         where st.branch_id = s.branch_id and st.key = 'sales'
           and (st.value ->> 'oil_change_master_fee_tyiyn') ~ '^[0-9]+$'),
        3000)
where s.master_fee_tyiyn = 0;
