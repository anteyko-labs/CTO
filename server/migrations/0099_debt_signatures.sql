-- Подпись клиента за долг на экране покупателя и подпись руководителя точки для документа
-- о долге (задача 008).

-- 1. Подпись клиента за чек в долг: ставится один раз вместе с чеком и не меняется.
create table sale_signatures (
    sale_id       uuid primary key references sales (id),
    signature_png bytea not null
        check (octet_length(signature_png) between 8 and 204800),
    created_at    timestamptz not null default now()
);

create trigger sale_signatures_immutable before update or delete on sale_signatures
    for each row execute function forbid_change();

-- 2. Подпись руководителя точки — печатается в документах о долге; меняет владелец.
create table debt_doc_signatures (
    branch_id     uuid primary key references branches (id),
    signature_png bytea not null
        check (octet_length(signature_png) between 8 and 204800),
    updated_at    timestamptz not null default now(),
    updated_by    uuid references users (id)
);
