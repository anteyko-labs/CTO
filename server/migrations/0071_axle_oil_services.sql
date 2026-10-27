-- Масло моста, редуктора и раздатки — не масло двигателя и не коробки (ADR-131, SPEC-36 §3).
-- Услуги с «мост» (с начала слова: «самостоятельно» — не мост), «редукт» или «раздатк» в названии,
-- стоящие на строке «Масло двигателя» или «Масло в коробке», переходят в «прочее» — то же правило,
-- что у новых услуг (`oil_book::service_slot_by_name`). «Бесплатно с нашим маслом» у них считается
-- от категории трансмиссионных масел (строка книжки «Масло в коробке» или «трансмис» в названии,
-- первая по времени заведения), а не моторных; такой категории нет — категория остаётся прежней.
-- Остальные услуги не меняются; повторный запуск ничего не меняет.
with gear as (
    select id from categories
    where kind = 'oil' and (book_slot = 'gear_oil' or lower(name) like '%трансмис%')
    order by created_at, id
    limit 1
)
update services s
set book_slot = 'other',
    oil_category_id = case
        when s.free_with_oil and not exists (
                select 1 from categories c
                where c.id = s.oil_category_id and c.kind = 'oil'
                  and (c.book_slot = 'gear_oil' or lower(c.name) like '%трансмис%'))
            then coalesce((select id from gear), s.oil_category_id)
        else s.oil_category_id
    end
where s.book_slot in ('oil', 'gear_oil')
  and (lower(s.name) ~ '(^|[^0-9a-zа-яё])мост' or lower(s.name) similar to '%(редукт|раздатк)%');
