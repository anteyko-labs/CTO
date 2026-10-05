---
type: spec
status: ready
tier: 4
stage: 1
updated: 2026-10-05
---

# SPEC-04. Касса: продажа и возврат

## Задача

Чек на вынос или в сервис с товарами, маслом канистрой и на розлив, услугами; кассир и мастер из справочника; ручная оплата наличными, картой, переводом, в том числе смешанная; возврат сторнирующим чеком.

## Продажа

`POST /sales`
```
{
  op_id, client_time,
  sale_type: takeaway | service,
  cashier_id, master_id?,           -- master обязателен при service
  comment?,
  lines: [
    {kind: piece,     product_id, qty,             unit_price_tyiyn},
    {kind: container, product_id, qty (канистры),  unit_price_tyiyn},
    {kind: pour,      product_id, qty (мл),        unit_price_tyiyn (за литр)},
    {kind: service,   service_id, qty,             unit_price_tyiyn}
  ],
  payments: [{method: cash|card|transfer, amount_tyiyn}]
}
```

## Алгоритм

```
в run_operation:
  проверить: строки есть; cashier активен и is_cashier; при service — master активен и is_master,
             и есть хотя бы одна строка service; при takeaway строк service нет
  для каждой строки:
     piece:     товар unit=piece; units = qty;                 amount = qty · price;  list = sale_price
     container: товар unit=ml;    units = qty · container_ml;  amount = qty · price;  list = sale_price
     pour:      товар unit=ml;    units = qty;                 amount = div_round(qty · price, 1000); list = pour_price
     service:   units = 0;                                      amount = qty · price;  list = service.price,
                master_fee = qty · service.master_fee
     qty > 0, price ≥ 0
  total = Σ amount
  Σ payments = total, каждый платёж > 0, иначе 422
  sale = insert (number = next(sale_seq))
  для товарных строк:
     bp = branch_products for update
     cost = cost_of(units, bp...)
     apply_movement(product, −units, −cost, 'sale', sale.id)
     line.cost_tyiyn = cost
  insert lines, payments
  если unit_price ≠ list_price → журнал sale.price_override по строке
  audit sale.post
  ответ: чек (cost_tyiyn — только владельцу)
```

Остаток может уйти в минус: продажа проводится, товар помечается для проверки (инвариант 10).

## Возврат

`POST /sales/{id}/return {op_id, comment, lines: [{line_no, qty}], payments: [{method, amount_tyiyn}]}`

```
исходный чек должен быть kind=sale
для каждой строки возврата:
  уже_возвращено = Σ qty этой строки во всех возвратах
  qty ≤ исходное qty − уже_возвращено, иначе 422
  amount = div_round(исходный amount · qty, исходное qty)
  cost   = div_round(исходный cost · qty, исходное qty)
  master_fee аналогично
  товарная строка: units пропорционально; apply_movement(+units, +cost, 'sale_return')
return_sale: kind=return, reversal_of=исходный, суммы со знаком минус
Σ payments = Σ amount возврата
```

## Чтение

- `GET /sales?date=YYYY-MM-DD` (по Asia/Bishkek) → список с итогами и суммами по способам оплаты.
- `GET /sales/{id}` → чек со строками, платежами и возвратами.

## Клиент — экран «Касса»

- Переключатель «На вынос / В сервис». В сервисе — выбор мастера и кнопки услуг.
- Выбор кассира (запоминается до смены).
- Поле сканера всегда в фокусе: Enter после кода добавляет товар; повторный скан увеличивает количество.
- Поиск товара по тексту с фильтром по категории.
- Для масла выбор: «Канистра» или «Розлив, л» (ввод литров с шагом 0,1 → мл).
- Цена строки редактируется; изменённая цена подсвечивается.
- Итог, оплата: кнопки «Наличные», «Карта», «Перевод», «Смешанная». Для наличных — поле «получено» и сдача.
- После проведения — номер чека, кнопка «Печать чека» (HTML-шаблон), очистка.
- Количество меняется кнопками −/+ (шаг 1 или 0,5 л для розлива); строка, где продают больше остатка, помечается (продажа не запрещается, инвариант 10).
- Единственный активный кассир или мастер выбирается автоматически; единственная услуга добавляется при переключении в «В сервис».
- Для наличных — кнопки «Без сдачи» и ближайших купюр, крупный показ сдачи; Enter в поле «Получено» или Ctrl+Enter проводят чек.
- Незавершённый чек хранится в sessionStorage вкладки и не теряется при переходе на другой экран.
- «Чеки за день»: переключение дней стрелками, выручка, число чеков, средний чек, возвраты, суммы по способам оплаты; список, просмотр, оформление возврата.

## Критерии приёмки

- [ ] Розлив 1500 мл по 333,33 с/л → 49 999,5 тыйын округляется до 50 000 (500,00 с).
- [ ] Сумма оплат не равна итогу → 422, ничего не записано.
- [ ] Продажа в сервис без мастера → 422.
- [ ] Продажа списывает остаток и стоимость, себестоимость строки фиксируется.
- [ ] Возврат больше проданного → 422; частичный возврат возвращает пропорциональную стоимость.
- [ ] Администратор не получает `cost_tyiyn` в ответе.
- [ ] Повтор `op_id` не создаёт второй чек.
- [ ] Продажа при нулевом остатке проходит и помечает товар.
