-- Задача 034: станция документов на моноблоке кассы — четвёртый вид станции очереди печати (`doc`),
-- своё окно Chrome с принтером HP. Задание несёт готовый документ; после печати HTML стирается.
alter table label_jobs drop constraint label_jobs_size_check;
alter table label_jobs add constraint label_jobs_size_check check (size in ('big', 'small', 'book', 'doc'));
alter table label_stations drop constraint label_stations_size_check;
alter table label_stations add constraint label_stations_size_check check (size in ('big', 'small', 'book', 'doc'));
alter table label_jobs add column doc_title text;
alter table label_jobs add column doc_html text;
alter table label_jobs add constraint label_jobs_doc_check check (size = 'doc' or (doc_title is null and doc_html is null));
