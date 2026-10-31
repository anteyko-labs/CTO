-- Задача 003 Н10: права кассира без входа и характеристики объектов (ADR-152, ADR-153).

-- Что кассиру нельзя на кассе: ключи прав групп «Касса» и «Деньги». Пусто — «как у входа».
-- Пока открыта смена этого кассира, у вошедшего администратора действует пересечение прав входа
-- и кассира. Список проверяет сервер (auth::PERMISSIONS): новое право группы сюда попадает
-- без миграции.
alter table employees add column cashier_denied text[] not null default '{}';

-- «Скрыть категорию»: пропадает из выбора; скрыть можно только без товаров в продаже.
alter table categories add column archived boolean not null default false;

-- Реквизиты контрагента (поставщика): адрес, банк и счёт. ИНН, вид и срок оплаты уже есть.
alter table parties add column address text not null default '';
alter table parties add column bank text not null default '';
alter table parties add column bank_account text not null default '';
