-- Личный вход сотрудника (задача 005 Н1, ADR-170): вход в программу связан с карточкой сотрудника
-- того же филиала. Связь необязательная: общий вход кассы и вход владельца — без сотрудника.
-- У сотрудника не больше одного входа. Филиал совпадает — составной внешний ключ: перевести вход
-- в другой филиал, не отвязав сотрудника, нельзя.
alter table employees add constraint employees_id_branch_uq unique (id, branch_id);

alter table users add column employee_id uuid;
alter table users add constraint users_employee_fk
    foreign key (employee_id, branch_id) references employees (id, branch_id);
create unique index users_employee_uq on users (employee_id) where employee_id is not null;
