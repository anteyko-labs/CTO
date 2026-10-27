//! Масляная книжка клиента в боте (SPEC-36 §6, ADR-119). Книжка машины — коротко: «Сейчас в
//! машине» по строкам книжки — что залито или поставлено, дата, пробег и следующая замена
//! (только пробег, ADR-115). История — кнопкой «📜 Прошлые ТО»: одно ТО (все записи книжки за
//! одну дату) за сообщение, от нового к старому, кнопкой «⬅ Показать до …». В ТО — строки
//! книжки, услуги чека, мастер и что он рекомендовал; себестоимости нет (ADR-061).

use std::collections::HashMap;

use chrono::NaiveDate;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::oil_book::{OilRecord, VehicleBook, books, now_lines, slot_label};
use crate::error::AppResult;

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

/// Книжка машины коротко: заголовок и «Сейчас в машине» по строкам книжки (SPEC-36 §6).
pub fn car_book(b: &VehicleBook, head: String) -> String {
    let now = now_lines(b, "  ");
    if now.is_empty() {
        return format!("{head}\nЗамен на точке пока не было.");
    }
    let mut lines = vec![head, "Сейчас в машине:".to_string()];
    lines.extend(now);
    lines.join("\n")
}

/// Одно ТО машины: все записи книжки за одну дату.
pub struct PastVisit {
    pub text: String,
    /// Дата этого ТО — курсор кнопки «⬅ Показать до».
    pub date: NaiveDate,
    /// Есть записи раньше этой даты.
    pub earlier: bool,
}

/// Дата ТО, которое показать: последняя, а с `before` — последняя строго раньше неё.
pub fn visit_date(b: &VehicleBook, before: Option<NaiveDate>) -> Option<NaiveDate> {
    b.records
        .iter()
        .map(|r| r.change_date)
        .filter(|d| before.is_none_or(|x| *d < x))
        .max()
}

/// Записи ТО за дату (свежие сверху, как в книжке).
pub fn visit_records(b: &VehicleBook, date: NaiveDate) -> Vec<&OilRecord> {
    b.records.iter().filter(|r| r.change_date == date).collect()
}

/// ТО за дату: «📜 09KG999ZXZ — ТО 26.10.2026 · 250 000 км», строки (что залили и поставили),
/// услуги, мастер, «Мастер рекомендовал». Следующей замены у строк нет — она в книжке машины.
pub fn past_visit(b: &VehicleBook, date: NaiveDate, x: &HashMap<Uuid, Extras>) -> PastVisit {
    let recs = visit_records(b, date);
    let mut head = format!("📜 {} — ТО {}", b.plate, date.format("%d.%m.%Y"));
    if let Some(r) = recs.iter().find(|r| r.mileage.is_some())
        && let Some(m) = r.mileage
    {
        head.push_str(&format!(
            " · {}",
            crate::domain::mileage::text(m, &r.mileage_unit)
        ));
    }
    let mut lines = vec![head];
    let mut services: Vec<String> = Vec::new();
    let mut masters: Vec<String> = Vec::new();
    let mut declined: Vec<&str> = Vec::new();
    for r in &recs {
        for l in &r.lines {
            lines.push(format!("  {}: {}", slot_label(&l.slot), l.text));
        }
        if let Some(e) = r.sale_id.and_then(|s| x.get(&s)) {
            // Услуга замены масла без своего масла уже стоит строкой «Масло двигателя».
            for sv in &e.services {
                if !r.lines.iter().any(|l| l.text == *sv) && !services.contains(sv) {
                    services.push(sv.clone());
                }
            }
            if let Some(m) = &e.master
                && !masters.contains(m)
            {
                masters.push(m.clone());
            }
        }
        if !r.declined.is_empty() && !declined.contains(&r.declined.as_str()) {
            declined.push(&r.declined);
        }
    }
    if !services.is_empty() {
        lines.push(format!("  Услуги: {}", services.join("; ")));
    }
    if !masters.is_empty() {
        lines.push(format!("  Мастер: {}", masters.join(", ")));
    }
    if !declined.is_empty() {
        lines.push(format!("  Мастер рекомендовал: {}", declined.join("; ")));
    }
    let earlier = visit_date(b, Some(date)).is_some();
    if !earlier {
        lines.push(String::new());
        lines.push("Это первая замена на точке.".into());
    }
    PastVisit {
        text: lines.join("\n"),
        date,
        earlier,
    }
}

/// Книжка клиента — ответ бота по умолчанию: баллы и «Сейчас в машине» по каждой машине
/// (до 20 машин); история — кнопкой «📜 Прошлые ТО».
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
    let mut parts: Vec<String> = list
        .iter()
        .take(BOOK_CARS)
        .map(|b| car_book(b, car_head(b)))
        .collect();
    if list.len() > BOOK_CARS {
        parts.push(format!(
            "… и ещё {} — откройте «🚗 Мои машины» и напишите номер машины.",
            list.len() - BOOK_CARS
        ));
    }
    if list.iter().any(|b| !b.records.is_empty()) {
        parts.push("Что делали раньше — кнопка «📜 Прошлые ТО».".into());
    }
    Ok(format!("{bonus}\n\n{}", parts.join("\n\n")))
}
