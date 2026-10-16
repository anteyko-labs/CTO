-- Несколько смен в день по очереди (SPEC-27, ADR-071): учётный день у смен дня общий,
-- одновременно открыта не больше одной — это проверяется при открытии под блокировкой кассы.
alter table shifts drop constraint shifts_branch_id_account_id_business_date_key;

-- Смена по времени: открытая в момент чека, последняя перед ним, следующая после него.
create index shifts_account_opened_idx on shifts (branch_id, account_id, opened_at);
