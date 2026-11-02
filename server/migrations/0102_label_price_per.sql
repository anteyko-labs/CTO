-- Задача 021: цена на этикетке — за шт, за литр или за метр; выбирается при печати и едет с заданием
-- на станцию. Без выбора (старые телефоны) — «за шт», как печаталось раньше.
alter table label_jobs add column price_per text not null default 'piece'
    check (price_per in ('piece', 'liter', 'meter'));
