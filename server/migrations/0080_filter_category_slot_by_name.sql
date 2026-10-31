-- Строка книжки категорий фильтров, заведённых до правила «по названию» (задача 003 Н5): у
-- «Фильтры топливные» строки не было, и по виду категории она считалась масляным фильтром. Пока
-- товар пишется по своему «Типу», это не видно; выключил владелец «Тип» у категории (ADR-{B3.2}) или
-- у товара типа нет — топливный фильтр уходил в книжку масляным, на срок масла. Теперь — как у
-- новой категории (`catalog::book_slot_by_name`): воздушный, салонный, топливный, масляный по
-- названию. Строку, выбранную владельцем, и категории без подсказки в названии не трогаем; записи
-- книжки не меняются (I4).
update categories
   set book_slot = case
         when lower(name) like '%воздуш%' then 'air_filter'
         when lower(name) like '%салон%' then 'cabin_filter'
         when lower(name) like '%топлив%' then 'fuel_filter'
         else 'oil_filter'
       end
 where kind = 'filter'
   and book_slot is null
   and (lower(name) like '%воздуш%' or lower(name) like '%салон%'
        or lower(name) like '%топлив%' or lower(name) like '%масл%');
