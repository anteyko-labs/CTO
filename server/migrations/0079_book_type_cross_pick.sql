-- Масляная книжка и кросс-номера (задача 002 П10, задача 003 Н5; ADR-{B3.1}, ADR-{B3.2}).

-- 1. «Тип» фильтра выключен у категории (выключатель характеристики, `enabled: false`) — строка
--    книжки её товаров берётся от категории, а не от типа товара (ADR-072). Нет характеристики или
--    она включена — как было: по типу.
create function category_by_type(attrs jsonb) returns boolean language sql immutable as $$
    select not coalesce(
        jsonb_typeof(attrs) = 'array'
        and exists (select 1 from jsonb_array_elements(attrs) a
                    where a ->> 'key' = 'filter_type' and a ->> 'enabled' = 'false'),
        false)
$$;

-- 2. Цель следующей замены, которую поставил сервер (кассир её не прислал и не очищал): пробег
--    сейчас + интервал строки (ADR-{B3.1}). Как цель кассира показывается везде, но «Исправить» и
--    «Изменить оплату» её не переносят — новый чек считает её заново.
alter table oil_change_items add column target_auto boolean not null default false;
alter table oil_change_items add constraint oil_change_items_target_auto_check
    check (not target_auto or target_set);

-- 3. Кросс-номер, подобранный сервисом разбора (ADR-{B3.2}): пометка «подобрано автоматически»
--    в карточке; номер удаляется в карточке, как любой другой.
alter table product_cross_numbers add column auto boolean not null default false;

-- 4. Подборы кросс-номеров: каждое обращение к сервису — строка (дневной лимит филиала, повтор
--    `op_id` возвращает тот же ответ, I8). Строки не удаляются.
create table cross_picks (
    id            uuid primary key,
    op_id         uuid not null unique,
    branch_id     uuid not null references branches,
    product_id    uuid not null references products,
    user_id       uuid not null references users,
    status        text not null default 'pending' check (status in ('pending', 'done', 'failed')),
    -- Что предложил сервис и что записано в карточку (режим «записывать сразу»).
    found         jsonb not null default '[]',
    added         jsonb not null default '[]',
    error         text,
    input_tokens  bigint not null default 0,
    output_tokens bigint not null default 0,
    response      jsonb,
    created_at    timestamptz not null default now(),
    finished_at   timestamptz
);
create index cross_picks_branch_day_idx on cross_picks (branch_id, created_at);
create index cross_picks_product_idx on cross_picks (product_id, created_at);
