-- Старый долг поставщику из тетради (задача 003 Н15, ADR-{g1.1}). Запись `party_ledger` вида
-- `debt` с документом `old_debt` и отрицательной суммой (мы должны), учётный день — дата долга.
-- Номер накладной поставщика, по которой остался долг, хранится у записи: по нему долг узнают в
-- карточке, окне оплаты и акте сверки. Прежние записи — без номера. Право — прежнее `old_debts`.
alter table party_ledger
    add column supplier_doc text not null default ''
        constraint party_ledger_supplier_doc_len check (char_length(supplier_doc) <= 100);
