# 006 — товар в килограммах и метрах

Статус: утверждено (02.11.2026, владелец: «внеси побыстрее», «решай сам»)

## Зачем
Владелец 02.11.2026: «при ревизии или приёмке товар ещё может быть в метрах и кг — внеси побыстрее». Ранее: «если возникнут вопросы — решай сам». Сейчас 'g' (граммы) есть только у карточки «Аккумуляторы б/у (на вес)», и выбрать его в карточке товара нельзя. Метров нет совсем.

**Быстрая правка кг уже сделана раньше этой задачи (ветка `save-006-kg`, ADR-174, миграция `0095_kg_quick.sql`, тесты `units_kg_quick.rs`, `units-kg.spec.ts`).** В неё вошло: пометка лома `products.system_role = 'battery_scrap'` и поиск лома по пометке; «в килограммах» в карточке и при создании товара; смена шт↔кг и л↔кг только без остатка; «Завести» из Умага строки «кг» → 'g'. Задача 006 добавляет метры и всё, чего быстрая правка не покрыла. Сборщик в начале и перед концом делает `git merge save-006-kg`. Конфликты решать по смыслу, правка кг первична.

## Что меняется
- **К1 Единица в карточке и окно смены единицы (ADR-175, уточнение ADR-139).** «Единица»: в штуках / в литрах / в килограммах / в метрах. У нового товара — любая. У заведённого — окно «Единица и объём канистры» (только владелец, `product_units.rs::plan`). Правила:
  - без остатка во всех филиалах — любая смена;
  - с остатком — шт↔кг и шт↔м с коэффициентом «1 шт = X кг/м» (новое поле `piece_qty`, тысячные: г/мм). Пересчёт: шт→кг/м — mul=X, div=1, цена ×1000/X; кг/м→шт — mul=1, div=X, цена ×X/1000. Если не делится нацело — понятный отказ, как у литров;
  - кг↔м, кг↔л, м↔л с остатком — отказ «обнулите остаток ревизией». Остаток проверяется после чтения строк, по всем филиалам.

  - запреты ADR-139 остаются для всех пар, и без остатка тоже: черновик ревизии, ожидающая приёмка, нерешённый брак, продажа без цены закупки → 409; блокировка `for update`/`for key share` и `ensure_seen_unit` — как есть.

  Смену разных размерностей помечать в `product_unit_changes.reset`. По документу раньше такой смены `units_now` даёт 409 «поправьте ревизией» (возврат по старому чеку и т. п.). `reset = true` — у любой смены без коэффициента между разными единицами, кроме ml↔piece по `piece_ml` и смены канистры. Сюда же входят шт↔кг и л↔кг без остатка из быстрой правки. Если она пишет mul=div=1 без пометки, привести её код к этому правилу. При `reset`:
  - цена, последняя закупка (`last_cost`/`last_cost_qty`) и цена владельца переводятся по правилу из «Решено» (канистра → ×1000/c, иначе то же число за новую единицу цены);
  - минимальный остаток → 0;
  - ставка баллов «за единицу» — то же число;
  - правило подарка, где товар — условие: включённое → 422 «выключите правило подарка …», выключенное → `needs_fix` (ADR-149);
  - всё было → стало — в журнал `product.unit_change`.

  Товар-подарок хоть одного правила (`gift_rule_items.gift_product_id`, включённого или нет) в g/mm не переводится: 422 «товар — подарок в правиле …: уберите его из подарков». Так держится «подарком кг/м быть не может». При смене с коэффициентом порог правила-условия, минимальный остаток, закупка и цена владельца переводятся по ADR-139 (`mul`/`div`, цена — `price_mul`/`price_div`). Ставка баллов — по формуле ADR-139: у g/mm 1000 единиц учёта в единице правила. Карточке с `system_role` единицу не менять, архивировать и объединять её нельзя (`catalog.rs:1747,2397`). `unit_words`/`unit_into`/`per_unit`/`unit_word` знают g и mm. Веб: `lib/unitChange.ts`, `ProductUnitModal.tsx` (кг/м, поле коэффициента, текст отказа сервера), `ProductFormModal.tsx`:
  - 4 варианта единицы;
  - начальное значение из `preset.unit`;
  - минимальный остаток у ml|g|mm — тысячными;
  - подписи «Цена продажи за кг/м», закупка «за кг/м»;
  - блок «Изменить» скрывать по `system_role`, а не по 'g'.
- **К2 Лом аккумуляторов.** Пометку и поиск по ней сделала быстрая правка. Здесь только проверить, что пометка — единственный признак лома везде. Касса: подсказка «средняя закупка … дешевле не продать» (`Cashier.tsx:2556`) — только у строки товара лома (product_id из `/batteries`), а не у любой 'weight'.
- **К3 Метры 'mm' везде, где 'g' (ADR-175, расширение I1: длина — мм).** Миграция `0096_meters.sql`:
  - `products_unit_check` + 'mm';
  - `sale_lines_kind_check` и проверка kind у `cancelled_lines` (0057) + 'length';
  - `product_unit_changes` from/to unit → piece/ml/g/mm (безымянные CHECK из 0077 сносить поиском по `pg_get_constraintdef`, как 0078/0092) и столбец `reset boolean not null default false`;
  - заполнить `reset = true` у уже записанных смен, где `from_unit <> to_unit` и пара не ml↔piece. До 0096 с коэффициентом шла только ml↔piece, значит всё с 'g' — смены быстрой правки без остатка;
  - tier2 «последняя — 0096».

  Сервер:
  - `catalog::price_unit` ('g'|'mm' → 1000) — единственная функция «единиц в цене». Ею заменить дубли: `receipts.rs:678,1207,1602`, `receipt_drafts.rs:1648`, `deliveries.rs:176`, `loyalty.rs:242`, `product_units.rs:563`;
  - исправить и для 'g' цену за 1 г вместо кг: `catalog.rs:706` (last_purchase/avg_cost), `catalog.rs:1663` (закупка нового товара, по ней оценивается излишек ревизии), `refs.rs:1676`. В том же коммите убрать ×1000 на клиенте (`purchasePrice.ts:127`, `receiptLines.ts:174`);
  - `create_product_tx` принимает 'mm' (без канистры);
  - итоги склада — поле `mm` (`catalog.rs:989,1012,1104`), `unit_rank` 'mm' → 3;
  - `domain/qty.rs`: qty_text 'mm' → «1,25 м», sale_qty_text 'length';
  - `money.rs` — розлив у mm нет (тест).

  Веб, `lib/units.ts` — единый `StockUnit` и исчерпывающий switch с `never`, без молчаливого «шт»: `isMilli`, `unitShort`, `perWord`, `parseQty`/`qtyInputText` (запятая или точка, ≤ 3 знаков), `formatMeters` в `format.ts`. Дубли (`defects.ts`, `ProductPicker.stockText`, `pnl.unitsText`, qtyText в `ReceiptView`/`StockInView`/`Income`/`ShiftReport`) свести к нему. Типы `types.ts` (Product, ReceiptLine + 'g', SaleLineKind + 'length', CategoryTotal.mm, `system_role`).
- **К4 Ревизия.** Сервер уже принимает целые г/мм. Нужны: `Revision.tsx` parseCount/countInput через `parseQty` (сейчас «1,25 м» отвергается, «2» = 2 мм); подписи поля «+ м», «Пересчитано, м»; текст 770 «на вес, в метрах или масло без тары». Скан кг/м-товара не прибавляет — открывает ввод в «+ добавить». Несколько входов: дробные прибавки складываются на сервере (ADR-173). `revisionCount.qtyInUnits` — `milli` для g→g и mm→mm. `RevisionNewProduct.tsx` — единица из Умага или выбор, «На полке, кг/м», `unit` в теле `new-product`.
- **К5 Приход, приёмка, черновики, доставки, возвраты.**
  - `receiptLines.ts` (lineCalc/nextCount/unitCost/lastPer — milli для g|mm);
  - `ReceiptNew.tsx` (подписи «М», «Цена за м»), `ReceiptView.tsx` (кг/м-строка «12500 шт» и 12 500 этикеток → `countText`, этикеток 1);
  - `StockInView.tsx`, `stockIn.ts`, `receiptFix.ts`, `docFix.ts`, `supplierReturn.ts`, `SupplierReturns.tsx`, `SupplierCard.tsx`;
  - приёмка SPEC-39: `acceptances.rs:1021,1178`, `lib/acceptances.ts` (qtyWords/inputUnit 'mm' → «м»), текст `Acceptance.tsx:531`;
  - черновики по фото: `receipt_drafts.rs::suggest` и `receiptDrafts.ts::suggestFor` — для g/mm count = qty_milli, сумма одним округлением, measure 'm'. Если единица строки накладной не «кг»/«м» — заметка «проверьте единицу».
- **К6 Касса.**
  - `sales.rs`: ('length','mm') в `prepare_line` (и `was` 535), 'length' в `check_discounts` 803, тексте 860, `price_override` 1858, `no_receipt_return_tx` 3330/3341;
  - `cancelled_lines.rs:90,131`, `notifications.rs:424`, `oil_book.rs:505,530` (weight/length не складывать как штуки);
  - веб — `isMilliKind` = pour|weight|length в `Cashier.tsx`, `discount.ts`, `DiscountModal.tsx`, `saleFix.ts`, `SaleView.tsx` (+ «/кг», «/м» в цене), `NoReceiptReturn.tsx`, `salePrint.ts`, `OfflineBar.tsx`, `debtDocs.ts`, `cabinetExport.ts`, `shiftReportExport.ts`, `ShiftReport.tsx`, `nextChange.ts`;
  - подписи «Длина, м», «Цена за м», клавиатура decimal 3 знака, шаг ±0,5;
  - скан кг/м-товара в кассе ставит пустое количество и фокус в поле. Повторный скан не прибавляет, только снова фокусирует (сейчас 1 кг → «1001»);
  - аналоги (`analogs.rs`, `GET /products/{id}/analogs`; `Cashier.tsx:2551` «{stock_qty} шт», у масла сейчас тоже «8000 шт»). Отдавать `unit`/`container_ml`, на экране — `stockText`. `swapToAnalog` (850) берёт kind по единице аналога и сбрасывает количество, если единица другая. Иначе строка 'piece' с м-товаром получит 409;
  - чек из очереди без сети, где вид строки не подходит к единице товара ('piece'/'weight' у mm-товара — старый бандл не знает 'mm'), отклоняется, как в ADR-139/ADR-059. В инструкцию выката: обновить кассы до того, как заведут первый м-товар.
- **К7 Склад, Excel, печать, ценники, прибыль, бот, помощник, баллы.**
  - `stockSheet.ts`, `exportCols.ts`, `exportTable.ts` (mm, scale 3, «м»), `Products.tsx`, `TopProducts.tsx`, `pnl.ts`, `labels.ts`;
  - ценник у g/mm — «за кг»/«за м»;
  - `points.ts`, `PointsRuleField.tsx`, `giftRule.ts`, `Gifts.tsx` (порог кг/м десятичный), `bot_drafts.rs:817,1835,1981`, `assistant.rs:1943`, doc `catalog.rs:97`, `gifts.rs:32,105`.
- **К8 Умаг.** Одна серверная функция сопоставления единицы (`old_catalog.rs`): кг/килограмм → 'g', м/метр → 'mm', л/литр → 'ml' только с канистрой из названия (иначе как сейчас), прочее → 'piece'. Отдавать её как `product_unit` в Hit/ProductUmag. `oldCatalog.presetFromHit` берёт её, а не своё правило. «Завести» в ревизии, кассе, приходе, «Сканере товаров» (`ProductIntake.tsx`, `Umag.tsx`) создаёт карточку в этой единице, цена и закупка — за кг/м. Правило `umag_by_measure`/`umag_measure_code` (ADR-113) не трогать. Импорт (Excel 03.11) единицу уже сохраняет — проверить «кг»/«м». По решению «г» = штука, поэтому `umagCodes.measureOf`/`umagPer`/`umagUnitWord` у «г/гр/грамм» дают « за г»/«грамм», а не « за кг». Иначе ревизия (`RevisionSearchHits.tsx:77`), `UmagLink.tsx`, `UmagCard.tsx` покажут цену за грамм как цену за кг. `isMeasureUnit`/`isMeasureCode` не менять. `UmagCard.tsx:99` («добавили бы всю тару») у товара g/mm не показывать.

## Не входит
Весы с выводом веса в программу и разбор весовых наклеек EAN «2…». Дроби у штук. Другие единицы (упаковки, пары, бухты). Приход кг/м через бот — отказ с текстом «на розлив, на вес или в метрах — внесите в программе». Подарок товаром в кг/м.

## Затронутые файлы
`server/migrations/0096_meters.sql`; `server/src/api/{catalog,product_units,analogs,sales,cancelled_lines,loyalty,gifts,receipts,receipt_drafts,deliveries,refs,acceptances,bot_drafts,assistant,notifications,oil_book,old_catalog}.rs`, `server/src/domain/{qty,money}.rs`; `web/src/lib/{units,format,types,unitChange,revisionCount,oldCatalog,umagCodes,receiptLines,receiptDrafts,stockIn,receiptFix,docFix,supplierReturn,defects,acceptances,discount,saleFix,debtDocs,cabinetExport,shiftReportExport,nextChange,stockSheet,exportCols,exportTable,pnl,purchasePrice,points,giftRule}.ts`, `web/src/pages/{Revision,ProductIntake,ReceiptNew,ReceiptView,StockInView,Cashier,SaleView,ShiftReport,Products,Stock,Income,TopProducts,Gifts,SupplierCard,Acceptance}.tsx`, `web/src/components/{ProductFormModal,ProductUnitModal,RevisionNewProduct,Umag,UmagCard,ProductPicker,UnknownCodeModal,NoReceiptReturn,DiscountModal,OfflineBar,SupplierReturns,PointsRuleField,GiftPicker,salePrint,labels}`; tier0 I1, tier2 (перечень единиц и видов строк), SPEC-02/03/04/09/11/15/19/27/35/39 — точечно, ADR-175.

## Проверка
- Сквозной `server/tests/units_kg_m.rs`:
  - заведение кг- и м-товара;
  - приход 2,5 м по цене за м → себестоимость и последняя закупка за м;
  - ревизия 1,25 + 0,75 = 2 м двумя входами, «Завести» 2500 г — излишек × цена за кг / 1000;
  - продажа 1,5 м (сумма, себестоимость, возврат части, скидка, ниже закупки, отмена строки 'length');
  - смена шт→м с коэффициентом, отказ при нецелом пересчёте, кг↔м с остатком → отказ, без остатка → проходит, возврат старого чека после такой смены → 409;
  - лом не путается с обычным кг-товаром, у лома смена единицы, архив и объединение запрещены;
  - Умаг «Завести» кг/м → g/mm;
  - запреты ADR-139 (черновик ревизии, приёмка, брак, продажа без цены) → 409 и при смене без остатка;
  - `reset`: шт→кг без коэффициента и кг→м пишут `reset = true`, минимальный остаток 0, включённое правило-условие → 422, выключенное → `needs_fix`; товар-подарок в g/mm → 422;
  - миграция 0096 на базе со сменой быстрой правки (шт→кг) проставляет ей `reset`, возврат старого чека после неё → 409;
  - чек со строкой 'piece' или 'weight' у mm-товара → 409, строка 'length' у g-товара → 409.
- Дополнить `points_units.rs` (порог подарка и ставка баллов при шт→кг/м), `acceptance.rs` («1,25 из 2 м»), `invoice.rs` («12,5 кг», «25 м»), `umag.rs`, `http.rs` (итоги склада mm, цена владельца при cost_pending для mm, карточка поставщика за кг), тесты `qty.rs`, `money.rs`, `cancelled_lines.rs`, `oil_book.rs` (weight/length не «шт»), `notifications.rs` (1615, «м»), `acceptances.rs` (qty_words «1,25 из 2 м»), `bot_drafts.rs` (минимум «1,5» у mm → 1500), `analogs.rs` (unit в ответе).
- Веб: `units.test.ts` (qtyText «1,25 м», parseQty, perWord, unitShort), а также `unitChange`, `revisionCount`, `acceptances`, `receiptLines`, `receiptDrafts`, `stockIn`, `receiptFix`, `supplierReturn`, `discount`, `saleFix`, `salePrint`, `stockSheet`, `exportCols`, `exportTable`, `oldCatalog`, `purchasePrice`, `pnl`, `points`, `giftRule`, `umagCodes` («г» → « за г»), `labels`/`labelText` (ценник «за кг»/«за м»), `defects`, `docFix`.
- e2e `web/e2e/units-kg-m.spec.ts`: завести товар в метрах → приход 3,5 м → ревизия двумя входами («+ 1,25», «+ 0,75» = 2 м) → продажа 1,5 м (печать «1,5 м × …/м», повторный скан не прибавляет) → возврат; шт→кг с коэффициентом; «Завести» строки Умага «м»; минимальный остаток «1,5» м в карточке сохраняется как 1,5 м; обычный кг-товар в кассе после открытия продажи лома — без подсказки «средняя закупка … дешевле не продать»; аналог в метрах вместо штучного — строка «Длина, м». `stage4`, `stage7`, `oil-units`, `units-kg` проходят без правок.
- После правки SQL — `cargo sqlx prepare -- --all-targets` на `avtodom_e2e*` и `server/.sqlx` в коммит; `python3 scripts/registry.py`.
- `./scripts/sdd-review.sh`, полный e2e на чистой `avtodom_e2e*` перед выкатом.

## Решено без вопроса
- Метры — отдельный вид строки 'length' (в строке чека нет единицы, печать и отчёты читают kind).
- Коэффициент — новое поле `piece_qty` в тысячных, `piece_ml` остаётся для литров.
- Цена при смене разных размерностей без остатка: канистра c мл → цена за кг/м = цена ×1000/c, иначе как есть.
- Подарком товар в кг/м быть не может: 422 и форма. Условием подарка — может, порог в кг/м.
- Умаг: «г/гр/грамм» → штуки (цена у Умага за грамм), «мл» — как сейчас, коды меры не переходят (ADR-113).
- Приход при скане кг/м-товара — «1 кг»/«1 м», следующий скан +1.
- Топ товаров по количеству — без изменений (как мл у масла). Отчёт по маслу в услугах (`reports.rs:1662-1676`, `serviceSale.oilAmountText`) строки weight/length не считает — тоже без изменений.
- Подтверждение кода литра Умага при привязке у g/mm-товара не спрашивать.
