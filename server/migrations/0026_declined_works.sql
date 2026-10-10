-- Отказанные работы (ADR-055): мастер рекомендовал, клиент отказался — напомним в следующий раз.
alter table oil_changes add column declined text not null default '';
