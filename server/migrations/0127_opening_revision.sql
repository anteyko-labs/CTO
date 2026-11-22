-- Начальный остаток филиала (задача 042, исправление 22.11.2026): ревизия «Остатки из Умага»
-- ставит товар, который уже был, — это не доход. Её движения (и переоценка по ней) не входят в
-- прибыль, отчёты дня и смены; остаток и стоимость склада — как есть.
alter table revisions add column opening boolean not null default false;

create function opening_doc(doc uuid) returns boolean
language sql stable as $$
  select exists (select 1 from revisions r where r.id = doc and r.opening)
$$;

-- Уже проведённые начальные ревизии: на пустой склад (у всех строк ожидалось 0) и до первой
-- продажи филиала — Основной № 1 (03.11.2026, 5,44 млн) и Айкос № 1 «Остатки из Умага».
update revisions r set opening = true
 where r.status = 'posted'
   and exists (select 1 from revision_results x where x.revision_id = r.id)
   and not exists (select 1 from revision_results x where x.revision_id = r.id and x.expected_qty <> 0)
   and not exists (select 1 from sales s where s.branch_id = r.branch_id and s.created_at < r.posted_at);
