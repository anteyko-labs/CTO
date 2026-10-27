#!/usr/bin/env bash
# Копия базы восстанавливается так же, как на точке (deploy/windows/restore.ps1:105-112): база со
# всеми миграциями и данными (в том числе список Умага `old_catalog`) → pg_dump -Fc → чистая база,
# владелец без прав суперпользователя → pg_restore --no-owner. Должно быть: код 0, во всех таблицах
# столько же строк, вычисляемые коды Умага те же.
#
# pg_dump ставит пустой search_path. Своя функция в вычисляемом столбце или индексе, которая
# внутри зовёт другую свою функцию без схемы, при восстановлении не находится — таблица не
# создаётся, данные пропадают. Вложенные вызовы — только со схемой: `public.umag_bare(...)`.
#
# Подключение суперпользователя — PGHOST, PGPORT, PGUSER, PGPASSWORD; программы PostgreSQL берутся
# из PATH и должны быть той же версии, что сервер базы. Роль и базы проверки скрипт создаёт сам и
# удаляет в конце. По умолчанию база собирается из server/migrations и проверочных строк; копию
# своей базы можно проверить так: SOURCE_DB=avtodom_copy deploy/check-restore.sh (база только
# читается).
#   PGHOST=localhost PGUSER=postgres PGPASSWORD=… deploy/check-restore.sh
set -euo pipefail

ROLE=avtodom_restore_check
ROLE_PASSWORD=restore-check-1
SRC=avtodom_restore_src
DST=avtodom_restore_dst
MIGRATIONS="$(cd "$(dirname "$0")/../server/migrations" && pwd)"
WORK="$(mktemp -d)"
DUMP="$WORK/check.dump"

super_sql() { psql -X -q -v ON_ERROR_STOP=1 -d postgres "$@"; }
as_role() { PGUSER="$ROLE" PGPASSWORD="$ROLE_PASSWORD" "$@"; }

cleanup() {
  dropdb --if-exists "$DST" > /dev/null 2>&1 || true
  [ -n "${SOURCE_DB:-}" ] || dropdb --if-exists "$SRC" > /dev/null 2>&1 || true
  super_sql -c "drop role if exists $ROLE" > /dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT

# Число строк каждой таблицы и вычисляемые коды Умага — для сравнения до и после.
snapshot() {
  psql -X -q -At -v ON_ERROR_STOP=1 -d "$1" <<'SQL'
select format('select %L || ''='' || count(*) from public.%I', table_name, table_name)
from information_schema.tables
where table_schema = 'public' and table_type = 'BASE TABLE'
order by table_name
\gexec
select format('old_catalog: %s строк, %s кодов, %s',
              count(*), coalesce(sum(cardinality(codes)), 0),
              md5(coalesce(string_agg(concat_ws('|', id, codes, keys, keys_bare, name_norm), ';' order by id), '')))
from old_catalog;
SQL
}

dropdb --if-exists "$DST" > /dev/null
super_sql <<SQL
do \$\$ begin
  if not exists (select 1 from pg_roles where rolname = '$ROLE') then
    create role $ROLE login password '$ROLE_PASSWORD' nosuperuser nocreatedb nocreaterole;
  end if;
end \$\$;
SQL
createdb -O "$ROLE" "$DST"

if [ -n "${SOURCE_DB:-}" ]; then
  SRC="$SOURCE_DB"
  pg_dump -d "$SRC" -Fc -f "$DUMP"
else
  dropdb --if-exists "$SRC" > /dev/null
  createdb -O "$ROLE" "$SRC"
  # Миграции — как сервер на точке: роль программы, каждая в своей транзакции, по порядку.
  for f in "$MIGRATIONS"/*.sql; do
    as_role psql -X -q -1 -v ON_ERROR_STOP=1 -d "$SRC" -f "$f" > /dev/null
  done
  # Строки Умага на каждое правило кодов: штрихкод, артикул, «доп. код» со словами и вязкостью,
  # строка меры (литр) с весовой наклейкой, нули впереди, строка без кодов; одна связана с товаром.
  as_role psql -X -q -v ON_ERROR_STOP=1 -d "$SRC" <<'SQL'
insert into branches (id, name, is_main) values ('00000000-0000-7000-8000-000000000001', 'Основной', true);
insert into categories (id, name, kind) values ('00000000-0000-7000-8000-000000000002', 'Масла', 'oil');
insert into products (id, category_id, name, unit, container_ml)
values ('00000000-0000-7000-8000-000000000003', '00000000-0000-7000-8000-000000000002',
        'Shell Helix HX7 10W-40 4 л', 'ml', 4000);
insert into product_barcodes (code, product_id, source)
values ('4607012345678', '00000000-0000-7000-8000-000000000003', 'umag');
insert into old_catalog (id, branch_id, name, barcode, article, extra_code, unit, product_id) values
  ('00000000-0000-7000-8000-000000000011', '00000000-0000-7000-8000-000000000001',
   'Shell Helix HX7 10W-40 4 л', '4607012345678', 'HX7-4L',
   '0888602305  U;SHELLHELIX CITY 5W30 1L;10W-40', 'шт', '00000000-0000-7000-8000-000000000003'),
  ('00000000-0000-7000-8000-000000000012', '00000000-0000-7000-8000-000000000001',
   'Масло в розлив', '2760275', '', '2760275010002;0012345', 'л', null),
  ('00000000-0000-7000-8000-000000000013', '00000000-0000-7000-8000-000000000001',
   'Свеча зажигания', '', 'BKR6E', 'NGK-7822', 'шт', null),
  ('00000000-0000-7000-8000-000000000014', '00000000-0000-7000-8000-000000000001',
   'Ёлочка «Ваниль»', '', '', '', '', null);
SQL
  as_role pg_dump -d "$SRC" -Fc -f "$DUMP"
fi

# Восстановление ровно как restore.ps1: чистая база владельца, --no-owner, от его имени.
if ! as_role pg_restore -d "$DST" --no-owner "$DUMP"; then
  echo "КОПИЯ НЕ ВОССТАНАВЛИВАЕТСЯ: pg_restore завершился с ошибкой" >&2
  exit 1
fi

before="$(snapshot "$SRC")"
after="$(snapshot "$DST")"
if [ "$before" != "$after" ]; then
  echo "КОПИЯ ВОССТАНОВИЛАСЬ НЕ ПОЛНОСТЬЮ" >&2
  diff <(printf '%s\n' "$before") <(printf '%s\n' "$after") >&2 || true
  exit 1
fi
codes="$(printf '%s\n' "$after" | tail -n 1)"
case "$codes" in
  "old_catalog: 0 строк"*) echo "В проверяемой базе нет строк Умага — проверка неполная" >&2; exit 1 ;;
esac
echo "Копия восстанавливается: $codes"
