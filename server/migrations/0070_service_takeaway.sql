-- Доливка продаётся на вынос (SPEC-36 §4, ADR-118). Услуга с флажком `takeaway` идёт в чек
-- «На вынос» без клиента, машины, пробега и книжки; мастер по желанию. Флажок ставится
-- действующим услугам со словом «долив» в названии, дальше его меняет владелец в «Услугах».
alter table services add column takeaway boolean not null default false;

update services set takeaway = true where active and name ilike '%долив%';
