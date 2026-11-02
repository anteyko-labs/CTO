-- Задача 007: каждый видит только свой пересчёт, «= моё» исправляет своё число строки.
-- Исправление вниз («пробил лишнее») — прибавка меньше нуля; «= итог» и «убрал» — как раньше.
alter table revision_line_adds drop constraint revision_line_adds_qty_check;
alter table revision_line_adds add constraint revision_line_adds_qty_check check (qty >= 0 or mode = 'add');
