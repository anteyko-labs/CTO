-- Задача 039, ADR-201: роль «Бухгалтер» — всё как у владельца, только чтение, без прибыли.
alter table users drop constraint if exists users_role_check;
alter table users add constraint users_role_check check (role in ('owner', 'admin', 'accountant'));
