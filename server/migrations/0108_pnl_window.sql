-- Период «Прибыли и убытков» с часами (задача 033, ADR-193): попадает ли документ в отбор.
-- `w_start` пустой — отбора нет. Конец не включается. Документ, проведённый в свой учётный день, —
-- по времени проведения; задним числом и без времени (`ts` пустой) — только если отбор покрывает
-- его день целиком: времени в своём дне у него нет.
create function pnl_in_window(d date, ts timestamptz, w_start timestamptz, w_end timestamptz)
returns boolean language sql immutable as $$
    select w_start is null or case
        when ts is not null and (ts at time zone 'Asia/Bishkek')::date = d
            then ts >= w_start and ts < w_end
        else (d::timestamp at time zone 'Asia/Bishkek') >= w_start
            and ((d + 1)::timestamp at time zone 'Asia/Bishkek') <= w_end
    end
$$;

-- То же для документов, у которых день — сам момент проведения.
create function pnl_in_window(ts timestamptz, w_start timestamptz, w_end timestamptz)
returns boolean language sql immutable as $$
    select w_start is null or (ts >= w_start and ts < w_end)
$$;
