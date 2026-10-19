-- Доработки по проверке на точке, часть 3, раздел B (SPEC-29 §11–23, ADR-081 … ADR-084).
-- Прежние данные не меняются: новые колонки пустые или со значением «как было».

-- 15. PIN кассира (ADR-081): хеш argon2, как у паролей; сам PIN нигде не хранится и не отдаётся.
-- Пусто — PIN не задан: смену на этого кассира откроет только владелец своим паролем.
alter table employees add column pin_hash text;
alter table employees add column pin_set_at timestamptz;
alter table employees add constraint employees_pin_set_check
    check ((pin_hash is null) = (pin_set_at is null));

-- 19. Права отдельных пользователей (ADR-082): владелец выдаёт администратору отдельное право.
-- Пока одно — «Перелив масла». Владельцу права не нужны: он может всё.
create table user_permissions (
    user_id    uuid not null references users,
    branch_id  uuid not null references branches,
    permission text not null check (permission in ('oil_transfer')),
    granted_by uuid not null references users,
    granted_at timestamptz not null default now(),
    primary key (user_id, permission)
);

-- 20. Имя и фамилия клиента-человека отдельно (ADR-083). `name` остаётся полным именем для
-- показа и поиска («Фамилия Имя»); у прежних записей поля пустые — дописываются при правке.
alter table parties add column first_name text not null default '';
alter table parties add column last_name text not null default '';

-- 21. Справочник марок и моделей машин (ADR-083): общий для всех филиалов, как категории
-- товаров; пополняется сам, когда владелец или администратор вписывает новую марку или модель.
create table car_makes (
    id         uuid primary key,
    name       text not null check (btrim(name) <> '' and char_length(name) <= 60),
    created_by uuid references users,
    created_at timestamptz not null default now()
);
create unique index car_makes_name_uq on car_makes (lower(name));

create table car_models (
    id         uuid primary key,
    make_id    uuid not null references car_makes,
    name       text not null check (btrim(name) <> '' and char_length(name) <= 60),
    created_by uuid references users,
    created_at timestamptz not null default now()
);
create unique index car_models_name_uq on car_models (make_id, lower(name));

insert into car_makes (id, name)
select gen_random_uuid(), m
from unnest(array[
    'Toyota', 'Lexus', 'Honda', 'Nissan', 'Infiniti', 'Mitsubishi', 'Subaru', 'Mazda', 'Suzuki',
    'Hyundai', 'Kia', 'Daewoo', 'Chevrolet', 'Volkswagen', 'Audi', 'Mercedes-Benz', 'BMW',
    'Lada', 'Ford', 'Opel', 'Renault', 'Skoda', 'Chery', 'Haval', 'Geely', 'УАЗ', 'ГАЗ'
]) as m;

insert into car_models (id, make_id, name)
select gen_random_uuid(), mk.id, x.model
from (values
    ('Toyota', array['Camry', 'Corolla', 'RAV4', 'Land Cruiser', 'Land Cruiser Prado', 'Highlander',
                     'Ipsum', 'Estima', 'Alphard', 'Prius', 'Avensis', 'Vitz', 'Yaris', 'Hilux',
                     '4Runner', 'Sienna', 'Mark II', 'Crown', 'Harrier', 'Noah', 'Voxy', 'Wish',
                     'Funcargo', 'Caldina']),
    ('Lexus', array['RX', 'LX', 'GX', 'ES', 'IS', 'GS', 'LS', 'NX']),
    ('Honda', array['Fit', 'Civic', 'Accord', 'CR-V', 'Odyssey', 'Stepwgn', 'Elysion', 'Stream',
                    'HR-V', 'Insight', 'Jazz', 'Pilot']),
    ('Nissan', array['X-Trail', 'Qashqai', 'Teana', 'Almera', 'Primera', 'Note', 'Tiida', 'Patrol',
                     'Murano', 'Juke', 'Serena', 'Cefiro', 'Maxima', 'Pathfinder']),
    ('Infiniti', array['FX35', 'QX56', 'QX60', 'QX70', 'G35']),
    ('Mitsubishi', array['Pajero', 'Pajero Sport', 'Outlander', 'Lancer', 'Delica', 'ASX', 'L200',
                         'Galant', 'Space Star', 'Chariot']),
    ('Subaru', array['Forester', 'Outback', 'Legacy', 'Impreza', 'XV', 'Tribeca']),
    ('Mazda', array['3', '6', 'CX-5', 'CX-7', 'CX-9', 'MPV', 'Demio', 'Premacy', 'Tribute']),
    ('Suzuki', array['Grand Vitara', 'Swift', 'SX4', 'Jimny', 'Escudo']),
    ('Hyundai', array['Sonata', 'Elantra', 'Avante', 'Solaris', 'Accent', 'Tucson', 'Santa Fe',
                      'Starex', 'Getz', 'Grandeur', 'Porter']),
    ('Kia', array['Rio', 'Cerato', 'K5', 'Optima', 'Sportage', 'Sorento', 'Carnival', 'Picanto',
                  'Morning', 'Soul', 'Bongo']),
    ('Daewoo', array['Matiz', 'Nexia', 'Lacetti', 'Gentra', 'Damas', 'Labo', 'Lanos', 'Spark']),
    ('Chevrolet', array['Cobalt', 'Spark', 'Lacetti', 'Captiva', 'Malibu', 'Cruze', 'Aveo', 'Niva',
                        'Tahoe', 'Equinox']),
    ('Volkswagen', array['Golf', 'Passat', 'Polo', 'Jetta', 'Tiguan', 'Touareg', 'Sharan',
                         'Transporter', 'Caddy', 'Vento']),
    ('Audi', array['A4', 'A6', 'A8', 'Q5', 'Q7', '80', '100']),
    ('Mercedes-Benz', array['C-Class', 'E-Class', 'S-Class', 'ML', 'GL', 'G-Class', 'Sprinter',
                            'Vito', '190']),
    ('BMW', array['3 Series', '5 Series', '7 Series', 'X3', 'X5', 'X6']),
    ('Lada', array['2106', '2107', '2110', '2114', 'Niva 4x4', 'Priora', 'Granta', 'Kalina',
                   'Vesta', 'Largus']),
    ('Ford', array['Focus', 'Mondeo', 'Fusion', 'Escape', 'Explorer', 'Transit', 'Ranger']),
    ('Opel', array['Astra', 'Vectra', 'Zafira', 'Omega']),
    ('Renault', array['Logan', 'Duster', 'Megane', 'Sandero']),
    ('Skoda', array['Octavia', 'Superb', 'Rapid']),
    ('Chery', array['Tiggo 4', 'Tiggo 7']),
    ('Haval', array['Jolion', 'H6']),
    ('Geely', array['Coolray', 'Monjaro']),
    ('УАЗ', array['Patriot', 'Hunter', '452']),
    ('ГАЗ', array['Газель', 'Волга 3110'])
) as s(make, models)
join car_makes mk on mk.name = s.make
cross join lateral unnest(s.models) as x(model);

-- 22. Пробег в км или милях (ADR-084): единица — у машины, по умолчанию км. Замена хранит
-- пробег, как его вписали (число и единица), и тот же пробег в км — по нему считаются интервалы.
alter table party_vehicles add column mileage_unit text not null default 'km'
    check (mileage_unit in ('km', 'mi'));
alter table oil_changes add column mileage_value integer check (mileage_value >= 0);
alter table oil_changes add column mileage_unit text not null default 'km'
    check (mileage_unit in ('km', 'mi'));
update oil_changes set mileage_value = mileage_km where mileage_km is not null;
