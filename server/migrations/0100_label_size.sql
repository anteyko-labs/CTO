-- Задача 014, ADR-182: два принтера этикеток на складе — большой и маленький (30×20 мм).
-- Задание и станция помечены размером; станция берёт только задания своего размера.
alter table label_jobs add column size text not null default 'big' check (size in ('big', 'small'));
alter table label_stations add column size text not null default 'big' check (size in ('big', 'small'));
drop index label_jobs_queue;
create index label_jobs_queue on label_jobs (branch_id, size, status, created_at);
