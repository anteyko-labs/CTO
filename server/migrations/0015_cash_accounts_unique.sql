-- Кассы филиала заводятся один раз даже при одновременном первом входе (SPEC-05).
create unique index cash_accounts_branch_name on cash_accounts (branch_id, name);
