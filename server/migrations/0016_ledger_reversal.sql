-- Сторно записи долга (SPEC-10): каждая запись сторнируется один раз.
create unique index party_ledger_reversal_once on party_ledger (reversal_of) where reversal_of is not null;
