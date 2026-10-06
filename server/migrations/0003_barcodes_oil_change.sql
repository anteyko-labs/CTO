-- Обязательные штрихкоды (ADR-026) и замена масла как отметка чека (ADR-027).

-- Ставка замены фиксируется в чеке и позже не пересчитывается (инвариант 3).
alter table sales add column master_fee_tyiyn bigint not null default 0;

-- Товарам без штрихкода выдаются внутренние коды: EAN-13 «22» + номер + контрольная цифра.
do $$
declare
    p    record;
    seq  bigint;
    body text;
    sum  integer;
    i    integer;
    d    integer;
begin
    for p in select id from products pr
             where not exists (select 1 from product_barcodes b where b.product_id = pr.id)
             order by created_at
    loop
        seq := nextval('internal_barcode_seq');
        body := '22' || lpad(seq::text, 10, '0');
        sum := 0;
        for i in 1..12 loop
            d := substr(body, i, 1)::integer;
            sum := sum + case when i % 2 = 1 then d else d * 3 end;
        end loop;
        insert into product_barcodes (code, product_id, internal)
        values (body || ((10 - sum % 10) % 10)::text, p.id, true);
    end loop;
end $$;
