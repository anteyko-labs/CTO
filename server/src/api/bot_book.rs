//! Масляная книжка клиента в боте целиком (SPEC-30 §10): вся история каждой машины, от новой
//! замены к старой. В записи — дата и пробег (и сумма своего чека, ADR-061), масло (название,
//! литры или канистры), фильтры и прочие работы по строкам книжки (SPEC-27), услуги чека,
//! мастер и что он рекомендовал. Записи разделены пустой строкой: длинная книжка уходит
//! несколькими сообщениями, и запись не рвётся между ними (`telegram::split_message`).

use std::collections::HashMap;

use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::oil_book::{OilRecord, VehicleBook, books, next_lines, record_lines};
use crate::domain::money::format_som;
use crate::error::AppResult;

/// Машин, у которых общий ответ клиенту показывает книжку целиком. У кого больше (фирма) —
/// последняя замена каждой, а вся история — в карточке машины («🚗 Мои машины»).
pub const FULL_BOOK_CARS: usize = 3;
/// Машин в общем ответе; остальные — по номеру или кнопкой «🚗 Мои машины» (ADR-063).
pub const BOOK_CARS: usize = 20;

/// Чек замены: мастер и услуги.
#[derive(Default)]
pub struct Extras {
    pub master: Option<String>,
    pub services: Vec<String>,
}

/// Мастер и услуги из чеков записей (по номеру чека). Запись из тетради чека не имеет.
pub async fn extras(
    conn: &mut PgConnection,
    records: &[&OilRecord],
) -> AppResult<HashMap<Uuid, Extras>> {
    let ids: Vec<Uuid> = records.iter().filter_map(|r| r.sale_id).collect();
    let mut out: HashMap<Uuid, Extras> = HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let masters = sqlx::query!(
        r#"select s.id, e.full_name as "master?"
           from sales s left join employees e on e.id = s.master_id
           where s.id = any($1)"#,
        &ids
    )
    .fetch_all(&mut *conn)
    .await?;
    for m in masters {
        out.entry(m.id).or_default().master = m.master.filter(|n| !n.trim().is_empty());
    }
    // Услуги чека: что сделали («Замена масла в коробке»). Товары попадают в книжку по строке
    // книжки своей категории (SPEC-27); категория «не в книжке» — решение владельца, её товары
    // и подарки здесь не показываются.
    let services = sqlx::query!(
        r#"select l.sale_id, l.qty, coalesce(nullif(l.name, ''), sv.name, '') as "name!"
           from sale_lines l join services sv on sv.id = l.service_id
           where l.sale_id = any($1) and not l.gift
           order by l.sale_id, l.line_no"#,
        &ids
    )
    .fetch_all(&mut *conn)
    .await?;
    for l in services {
        let text = if l.qty > 1 {
            format!("{} × {}", l.name, l.qty)
        } else {
            l.name
        };
        out.entry(l.sale_id).or_default().services.push(text);
    }
    Ok(out)
}

/// Запись книжки: «дата · пробег · сумма чека», строки книжки, услуги чека, мастер, рекомендация
/// мастера. Отступ — у всех строк после первой.
pub fn record_text(r: &OilRecord, x: Option<&Extras>) -> String {
    let mut head = vec![r.change_date.format("%d.%m.%Y").to_string()];
    if let Some(m) = r.mileage {
        head.push(crate::domain::mileage::text(m, &r.mileage_unit));
    }
    // Запись из тетради — без чека, суммы нет.
    head.push(r.sale_total_tyiyn.map_or_else(|| "—".into(), format_som));
    let mut lines = vec![head.join(" · ")];
    // По строкам книжки: масло (название, литры или канистры), фильтры, прочие работы (SPEC-27).
    lines.extend(record_lines(r, "  "));
    if let Some(x) = x {
        // Услуга замены масла без своего масла уже стоит строкой «Масло двигателя».
        let services: Vec<&str> = x
            .services
            .iter()
            .map(String::as_str)
            .filter(|o| !r.lines.iter().any(|l| l.text == *o))
            .collect();
        if !services.is_empty() {
            lines.push(format!("  Услуги: {}", services.join("; ")));
        }
        if let Some(m) = &x.master {
            lines.push(format!("  Мастер: {m}"));
        }
    }
    if !r.declined.is_empty() {
        lines.push(format!("  Мастер рекомендовал: {}", r.declined));
    }
    lines.join("\n")
}

/// «01KG123ABC (Toyota Camry)».
fn car_head(b: &VehicleBook) -> String {
    let car = [b.brand.as_str(), b.model.as_str()]
        .into_iter()
        .filter(|x| !x.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if car.is_empty() {
        b.plate.clone()
    } else {
        format!("{} ({car})", b.plate)
    }
}

/// Вся книжка машины: следующая замена и все записи от новой к старой, через пустую строку.
pub fn full_book(b: &VehicleBook, x: &HashMap<Uuid, Extras>, head: String) -> String {
    let mut first = vec![head];
    // Следующая замена по каждой строке книжки (SPEC-27).
    let next = next_lines(b, "  ");
    if !next.is_empty() {
        first.push("Следующая замена — что наступит раньше:".into());
        first.extend(next);
    }
    let mut blocks = vec![first.join("\n")];
    if b.records.is_empty() {
        blocks.push("Замен на точке пока не было.".into());
        return blocks.join("\n\n");
    }
    for (i, r) in b.records.iter().enumerate() {
        let text = record_text(r, r.sale_id.and_then(|s| x.get(&s)));
        blocks.push(if i == 0 {
            format!("Замены:\n{text}")
        } else {
            text
        });
    }
    blocks.join("\n\n")
}

/// Книжка клиента — ответ бота по умолчанию: баллы и книжки машин. До трёх машин — каждая
/// целиком; больше — последняя замена каждой (до 20 машин), вся история — в карточке машины.
pub async fn customer_book(conn: &mut PgConnection, party_id: Uuid) -> AppResult<String> {
    let bonus = format!(
        "⭐ Баллов: {} — подробнее кнопкой «⭐ Баллы»",
        crate::api::telegram::points(crate::api::loyalty::balance(conn, party_id).await?)
    );
    let list = books(conn, Some(party_id), None).await?;
    if list.is_empty() {
        return Ok(format!(
            "{bonus}\n\nМашин у вас пока не записано — назовите госномер кассиру при замене."
        ));
    }
    let full = list.len() <= FULL_BOOK_CARS;
    let shown: Vec<&VehicleBook> = list.iter().take(BOOK_CARS).collect();
    let records: Vec<&OilRecord> = if full {
        shown.iter().flat_map(|b| b.records.iter()).collect()
    } else {
        shown.iter().filter_map(|b| b.records.first()).collect()
    };
    let x = extras(conn, &records).await?;
    let mut parts: Vec<String> = shown
        .iter()
        .map(|b| {
            if full {
                return full_book(b, &x, car_head(b));
            }
            let Some(last) = b.records.first() else {
                return format!("{}\nЗамен пока не было.", car_head(b));
            };
            let mut lines = vec![
                car_head(b),
                format!(
                    "Последняя замена: {}",
                    record_text(last, last.sale_id.and_then(|s| x.get(&s)))
                ),
            ];
            let next = next_lines(b, "  ");
            if !next.is_empty() {
                lines.push("Следующая замена:".into());
                lines.extend(next);
            }
            lines.join("\n")
        })
        .collect();
    if list.len() > BOOK_CARS {
        parts.push(format!(
            "… и ещё {} — откройте «🚗 Мои машины» и напишите номер машины.",
            list.len() - BOOK_CARS
        ));
    }
    if !full {
        parts.push("Вся история машины — «🚗 Мои машины» → машина.".into());
    }
    Ok(format!("{bonus}\n\n{}", parts.join("\n\n")))
}
