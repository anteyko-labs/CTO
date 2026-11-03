-- Задача 027: станция книжки на моноблоке кассы — третий вид станции очереди печати (`book`),
-- своё окно Chrome со своим принтером (наклейки 58×40). Задание книжки несёт машину и чек в `book`.
alter table label_jobs drop constraint label_jobs_size_check;
alter table label_jobs add constraint label_jobs_size_check check (size in ('big', 'small', 'book'));
alter table label_stations drop constraint label_stations_size_check;
alter table label_stations add constraint label_stations_size_check check (size in ('big', 'small', 'book'));
alter table label_jobs add column book jsonb;
alter table label_jobs add constraint label_jobs_book_check check ((size = 'book') = (book is not null));
