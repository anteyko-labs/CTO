//! «Исправить» у документов владельца (SPEC-29 §24, ADR-085): исходный документ сторнируется
//! своей функцией сторно, верный проводится своей функцией проведения — одной операцией, одной
//! транзакцией, с одним `op_id`. Своих правил проведения здесь нет: только выбор функций, связь
//! «исправлен → новый» (`doc_corrections`) и то, что поменялось, словами.
//!
//! Документ для формы («черновик») и описание для сравнения строятся одной функцией [`load_doc`]
//! из проведённого документа — исходного и нового одинаково, поэтому сравнение честное.

use std::collections::{BTreeSet, HashMap};

use axum::extract::{Path, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::auth::{Ctx, CurrentUser};
use crate::domain::money::format_som;
use crate::domain::qty::{fixed3, liters, qty_text, sale_qty_text};
use crate::error::{AppError, AppResult, invalid};
use crate::ops::{self, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/corrections", routing::get(list).post(post))
        .route("/corrections/draft/{kind}/{id}", routing::get(draft))
        .route("/corrections/doc/{id}", routing::get(links))
}

/// Вид исправляемого документа: у каждого есть своё сторно и своё проведение.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Sale,
    Receipt,
    Expense,
    CashMovement,
    CashTransfer,
    Ledger,
    Accrual,
    Payout,
    StockIn,
    SupplierReturn,
    BatteryIntake,
    OilTransfer,
}

impl Kind {
    pub const ALL: [Kind; 12] = [
        Kind::Sale,
        Kind::Receipt,
        Kind::Expense,
        Kind::CashMovement,
        Kind::CashTransfer,
        Kind::Ledger,
        Kind::Accrual,
        Kind::Payout,
        Kind::StockIn,
        Kind::SupplierReturn,
        Kind::BatteryIntake,
        Kind::OilTransfer,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Sale => "sale",
            Kind::Receipt => "receipt",
            Kind::Expense => "expense",
            Kind::CashMovement => "cash_movement",
            Kind::CashTransfer => "cash_transfer",
            Kind::Ledger => "ledger",
            Kind::Accrual => "accrual",
            Kind::Payout => "payout",
            Kind::StockIn => "stock_in",
            Kind::SupplierReturn => "supplier_return",
            Kind::BatteryIntake => "battery_intake",
            Kind::OilTransfer => "oil_transfer",
        }
    }

    pub fn parse(s: &str) -> AppResult<Self> {
        Kind::ALL
            .into_iter()
            .find(|k| k.as_str() == s)
            .ok_or_else(|| invalid("этот документ не исправляется"))
    }

    /// Документ словами — для истории и уведомления.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Sale => "чек",
            Kind::Receipt => "приход",
            Kind::Expense => "расход",
            Kind::CashMovement => "внесение или изъятие",
            Kind::CashTransfer => "перемещение денег",
            Kind::Ledger => "оплата долга",
            Kind::Accrual => "начисление сотруднику",
            Kind::Payout => "выплата сотруднику",
            Kind::StockIn => "оприходование",
            Kind::SupplierReturn => "возврат поставщику",
            Kind::BatteryIntake => "приём аккумуляторов",
            Kind::OilTransfer => "перелив",
        }
    }
}

/// Что поменялось: строка сравнения «что — было — стало».
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct DiffItem {
    pub what: String,
    pub old: String,
    pub new: String,
}

/// Документ словами: поля и строки (товары) — для сравнения исходного с новым.
#[derive(Default, Debug)]
struct Desc {
    fields: Vec<(String, String)>,
    lines: Vec<DescLine>,
}

#[derive(Debug)]
struct DescLine {
    key: String,
    name: String,
    parts: Vec<(String, String)>,
}

impl Desc {
    fn field(&mut self, label: &str, value: impl Into<String>) {
        self.fields.push((label.to_string(), value.into()));
    }
}

const NONE: &str = "—";

fn or_none(s: Option<String>) -> String {
    s.filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| NONE.into())
}

fn date_text(d: NaiveDate) -> String {
    format!("{:02}.{:02}.{}", d.day(), d.month(), d.year())
}

/// Сравнение двух описаний: изменённые поля, затем строки — изменённые, убранные, добавленные.
/// Строки сопоставляются по товару (услуге): у одинаковых — по порядку.
fn diff(a: &Desc, b: &Desc) -> Vec<DiffItem> {
    let mut out = Vec::new();
    let get = |d: &Desc, label: &str| {
        d.fields
            .iter()
            .find(|(l, _)| l == label)
            .map(|(_, v)| v.clone())
    };
    let mut labels: Vec<&String> = a.fields.iter().map(|(l, _)| l).collect();
    for (l, _) in &b.fields {
        if !labels.contains(&l) {
            labels.push(l);
        }
    }
    for label in labels {
        let (old, new) = (get(a, label), get(b, label));
        if old != new {
            out.push(DiffItem {
                what: label.clone(),
                old: or_none(old),
                new: or_none(new),
            });
        }
    }
    let (ka, kb) = (keyed(a), keyed(b));
    let joined = |l: &DescLine| {
        l.parts
            .iter()
            .map(|(p, v)| format!("{p} {v}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    for (key, la) in &ka {
        match kb.iter().find(|(k, _)| k == key) {
            Some((_, lb)) => {
                for (label, old) in &la.parts {
                    let new = lb
                        .parts
                        .iter()
                        .find(|(l, _)| l == label)
                        .map(|(_, v)| v.clone());
                    if new.as_ref() != Some(old) {
                        out.push(DiffItem {
                            what: format!("{}: {}", la.name, label),
                            old: old.clone(),
                            new: or_none(new),
                        });
                    }
                }
            }
            None => out.push(DiffItem {
                what: format!("{}: строка", la.name),
                old: joined(la),
                new: "убрана".into(),
            }),
        }
    }
    for (key, lb) in &kb {
        if !ka.iter().any(|(k, _)| k == key) {
            out.push(DiffItem {
                what: format!("{}: строка", lb.name),
                old: "не было".into(),
                new: joined(lb),
            });
        }
    }
    out
}

/// Строки с ключом «товар#порядковый»: одинаковые товары сопоставляются по порядку.
fn keyed(d: &Desc) -> Vec<(String, &DescLine)> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    d.lines
        .iter()
        .map(|l| {
            let n = seen.entry(l.key.as_str()).or_insert(0);
            *n += 1;
            (format!("{}#{}", l.key, n), l)
        })
        .collect()
}

/// Проведённый документ: номер, данные для формы (в виде запроса проведения) и описание.
struct Doc {
    number: Option<i64>,
    draft: Value,
    desc: Desc,
}

fn method_text(m: &str) -> &str {
    match m {
        "cash" => "наличные",
        "card" => "карта",
        "transfer" => "перевод",
        "debt" => "в долг",
        "bonus" => "баллы",
        "battery" => "аккумулятор",
        other => other,
    }
}

async fn account_name(conn: &mut PgConnection, id: Option<Uuid>) -> AppResult<Option<String>> {
    let Some(id) = id else { return Ok(None) };
    Ok(
        sqlx::query_scalar!("select name from cash_accounts where id = $1", id)
            .fetch_optional(&mut *conn)
            .await?,
    )
}

/// Документ для формы и сравнения. Чужой филиал или нет такого — 404.
async fn load_doc(
    conn: &mut PgConnection,
    user: &CurrentUser,
    kind: Kind,
    id: Uuid,
) -> AppResult<Doc> {
    let branch_id = user.branch_id;
    match kind {
        Kind::Sale => load_sale_doc(conn, user, id).await,
        Kind::Receipt => load_receipt_doc(conn, branch_id, id).await,
        Kind::Expense => {
            let e = crate::api::expenses::load_expense(conn, branch_id, id).await?;
            let account_id =
                sqlx::query_scalar!("select account_id from expenses where id = $1", id)
                    .fetch_one(&mut *conn)
                    .await?;
            let mut desc = Desc::default();
            desc.field("Статья", e.article_name.clone());
            desc.field("Сумма", format_som(e.amount_tyiyn));
            desc.field("Откуда", or_none(e.account_name.clone()));
            desc.field("Дата", date_text(e.expense_date));
            desc.field("Комментарий", or_none(Some(e.comment.clone())));
            Ok(Doc {
                number: Some(e.number),
                draft: json!({
                    "article_id": e.article_id,
                    "amount_tyiyn": e.amount_tyiyn,
                    "source": "account",
                    "account_id": account_id,
                    "expense_date": e.expense_date,
                    "comment": e.comment,
                }),
                desc,
            })
        }
        Kind::CashMovement => {
            let m = sqlx::query!(
                r#"select m.kind, m.amount_tyiyn, m.account_id, m.comment, m.doc_type, a.name as account
                   from cash_movements m join cash_accounts a on a.id = m.account_id
                   where m.id = $1 and m.branch_id = $2"#,
                id,
                branch_id
            )
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(AppError::NotFound)?;
            let mut desc = Desc::default();
            desc.field(
                "Вид",
                match m.kind.as_str() {
                    "cash_in" => "внесение",
                    "cash_out" => "изъятие",
                    other => other,
                },
            );
            desc.field("Счёт", m.account.clone());
            desc.field("Сумма", format_som(m.amount_tyiyn.abs()));
            desc.field("Комментарий", or_none(Some(m.comment.clone())));
            Ok(Doc {
                number: None,
                draft: json!({
                    "account_id": m.account_id,
                    "kind": m.kind,
                    "amount_tyiyn": m.amount_tyiyn.abs(),
                    "comment": m.comment,
                }),
                desc,
            })
        }
        Kind::CashTransfer => {
            let t = sqlx::query!(
                r#"select t.number, t.from_account_id, t.to_account_id, t.amount_tyiyn, t.comment,
                          f.name as from_name, d.name as to_name
                   from cash_transfers t
                   join cash_accounts f on f.id = t.from_account_id
                   join cash_accounts d on d.id = t.to_account_id
                   where t.id = $1 and t.branch_id = $2"#,
                id,
                branch_id
            )
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(AppError::NotFound)?;
            let mut desc = Desc::default();
            desc.field("Откуда", t.from_name.clone());
            desc.field("Куда", t.to_name.clone());
            desc.field("Сумма", format_som(t.amount_tyiyn));
            desc.field("Комментарий", or_none(Some(t.comment.clone())));
            Ok(Doc {
                number: Some(t.number),
                draft: json!({
                    "from_account_id": t.from_account_id,
                    "to_account_id": t.to_account_id,
                    "amount_tyiyn": t.amount_tyiyn,
                    "comment": t.comment,
                }),
                desc,
            })
        }
        Kind::Ledger => load_ledger_doc(conn, branch_id, id).await,
        Kind::Accrual => {
            let a = sqlx::query!(
                r#"select a.employee_id, a.business_date, a.kind, a.amount_tyiyn, a.comment, a.doc_type,
                          e.full_name
                   from payroll_accruals a join employees e on e.id = a.employee_id
                   where a.id = $1 and a.branch_id = $2"#,
                id,
                branch_id
            )
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(AppError::NotFound)?;
            let mut desc = Desc::default();
            desc.field("Сотрудник", a.full_name.clone());
            desc.field(
                "Вид",
                match a.kind.as_str() {
                    "bonus" => "премия",
                    "penalty" => "удержание",
                    "monthly_salary" => "оклад",
                    other => other,
                },
            );
            desc.field("Сумма", format_som(a.amount_tyiyn.abs()));
            let draft = if a.kind == "monthly_salary" {
                let month = a.business_date.with_day(1).unwrap_or(a.business_date);
                desc.field("Месяц", format!("{:02}.{}", month.month(), month.year()));
                json!({
                    "type": "salary",
                    "employee_id": a.employee_id,
                    "month": month,
                    "amount_tyiyn": a.amount_tyiyn,
                })
            } else {
                desc.field("Дата", date_text(a.business_date));
                desc.field("Комментарий", or_none(Some(a.comment.clone())));
                json!({
                    "type": "accrual",
                    "employee_id": a.employee_id,
                    "kind": a.kind,
                    "amount_tyiyn": a.amount_tyiyn.abs(),
                    "business_date": a.business_date,
                    "comment": a.comment,
                })
            };
            Ok(Doc {
                number: None,
                draft,
                desc,
            })
        }
        Kind::Payout => {
            let p = sqlx::query!(
                r#"select p.number, p.employee_id, p.amount_tyiyn, p.account_id, p.comment, e.full_name
                   from payouts p join employees e on e.id = p.employee_id
                   where p.id = $1 and p.branch_id = $2"#,
                id,
                branch_id
            )
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(AppError::NotFound)?;
            let mut desc = Desc::default();
            desc.field("Сотрудник", p.full_name.clone());
            desc.field("Сумма", format_som(p.amount_tyiyn));
            desc.field("Откуда", or_none(account_name(conn, p.account_id).await?));
            desc.field("Комментарий", or_none(Some(p.comment.clone())));
            Ok(Doc {
                number: Some(p.number),
                draft: json!({
                    "employee_id": p.employee_id,
                    "amount_tyiyn": p.amount_tyiyn,
                    "source": "account",
                    "account_id": p.account_id,
                    // Исправляет владелец: сверх заработанного ему можно (аванс).
                    "advance": true,
                    "comment": p.comment,
                }),
                desc,
            })
        }
        Kind::StockIn => {
            let s = crate::api::stock_in::load_one(conn, branch_id, id).await?;
            // Цену продажи ставило само оприходование — только у таких товаров она в форме (как у
            // прихода: иначе исправление вернуло бы старую цену поверх поставленной позже).
            let priced: Vec<Uuid> = sqlx::query_scalar!(
                r#"select entity_id as "id!" from audit_log
                   where branch_id = $1 and action = 'product.prices' and entity_id is not null
                     and data ->> 'stock_in_id' = $2"#,
                branch_id,
                id.to_string()
            )
            .fetch_all(&mut *conn)
            .await?;
            let mut desc = Desc::default();
            desc.field("Стоимость", format_som(s.cost_tyiyn));
            desc.field("Комментарий", or_none(Some(s.comment.clone())));
            desc.lines = s
                .lines
                .iter()
                .map(|l| {
                    let mut parts = vec![
                        ("количество".into(), qty_text(l.qty, &l.unit)),
                        ("цена закупки".into(), format_som(l.price_tyiyn)),
                        ("сумма".into(), format_som(l.cost_tyiyn)),
                    ];
                    if priced.contains(&l.product_id)
                        && let Some(p) = l.sale_price_tyiyn
                    {
                        parts.push(("цена продажи".into(), format_som(p)));
                    }
                    DescLine {
                        key: l.product_id.to_string(),
                        name: l.product_name.clone(),
                        parts,
                    }
                })
                .collect();
            Ok(Doc {
                number: Some(s.number),
                draft: json!({
                    "comment": s.comment,
                    "lines": s.lines.iter().map(|l| json!({
                        "product_id": l.product_id,
                        "qty": l.qty,
                        "price_tyiyn": l.price_tyiyn,
                        "sale_price_tyiyn": if priced.contains(&l.product_id) { l.sale_price_tyiyn } else { None },
                    })).collect::<Vec<_>>(),
                }),
                desc,
            })
        }
        Kind::SupplierReturn => {
            let r = crate::api::supplier_returns::load_one(conn, user, id).await?;
            let mut desc = Desc::default();
            desc.field("Поставщик", r.supplier_name.clone());
            desc.field(
                "Итог",
                if r.settle == "money" {
                    "деньгами"
                } else {
                    "в счёт долга"
                },
            );
            desc.field("Счёт", or_none(r.account_name.clone()));
            desc.field("Комментарий", or_none(Some(r.comment.clone())));
            desc.lines = r
                .lines
                .iter()
                .map(|l| DescLine {
                    key: l.product_id.to_string(),
                    name: l.product_name.clone(),
                    parts: vec![
                        ("количество".into(), qty_text(l.qty, &l.unit)),
                        ("сумма".into(), format_som(l.amount_tyiyn)),
                    ],
                })
                .collect();
            Ok(Doc {
                number: Some(r.number),
                draft: json!({
                    "supplier_id": r.supplier_id,
                    "settle": r.settle,
                    "account_id": r.account_id,
                    "comment": r.comment,
                    "lines": r.lines.iter().map(|l| json!({
                        "product_id": l.product_id,
                        "qty": l.qty,
                        "amount_tyiyn": l.amount_tyiyn,
                    })).collect::<Vec<_>>(),
                }),
                desc,
            })
        }
        Kind::BatteryIntake => {
            let b = sqlx::query!(
                r#"select number, grams, price_per_kg_tyiyn, amount_tyiyn, comment
                   from battery_intakes where id = $1 and branch_id = $2"#,
                id,
                branch_id
            )
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(AppError::NotFound)?;
            let mut desc = Desc::default();
            desc.field("Вес", format!("{} кг", fixed3(b.grams)));
            desc.field("Цена за кг", format_som(b.price_per_kg_tyiyn));
            desc.field("Сумма", format_som(b.amount_tyiyn));
            desc.field("Комментарий", or_none(Some(b.comment.clone())));
            Ok(Doc {
                number: Some(b.number),
                draft: json!({
                    "grams": b.grams,
                    "price_per_kg_tyiyn": b.price_per_kg_tyiyn,
                    "comment": b.comment,
                }),
                desc,
            })
        }
        Kind::OilTransfer => {
            let t = sqlx::query!(
                r#"select t.number, t.from_product_id, t.to_product_id, t.qty_ml, t.comment,
                          f.name as from_name, d.name as to_name
                   from oil_transfers t
                   join products f on f.id = t.from_product_id
                   join products d on d.id = t.to_product_id
                   where t.id = $1 and t.branch_id = $2"#,
                id,
                branch_id
            )
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(AppError::NotFound)?;
            let mut desc = Desc::default();
            desc.field("Из", t.from_name.clone());
            desc.field("В", t.to_name.clone());
            desc.field("Объём", liters(t.qty_ml));
            desc.field("Комментарий", or_none(Some(t.comment.clone())));
            Ok(Doc {
                number: Some(t.number),
                draft: json!({
                    "from_product_id": t.from_product_id,
                    "to_product_id": t.to_product_id,
                    "qty_ml": t.qty_ml,
                    "comment": t.comment,
                }),
                desc,
            })
        }
    }
}

async fn load_sale_doc(conn: &mut PgConnection, user: &CurrentUser, id: Uuid) -> AppResult<Doc> {
    let s = crate::api::sales::load_sale(conn, user, id).await?;
    let h = sqlx::query!(
        r#"select business_date, contact_id, vehicle_id from sales where id = $1"#,
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    let book = sqlx::query!(
        r#"select mileage_value, mileage_unit, declined from oil_changes where sale_id = $1 limit 1"#,
        id
    )
    .fetch_optional(&mut *conn)
    .await?;
    let mut desc = Desc::default();
    desc.field("Учётный день", date_text(h.business_date));
    desc.field(
        "Вид",
        if s.sale_type == "service" {
            "в сервис"
        } else {
            "на вынос"
        },
    );
    desc.field("Кассир", s.cashier_name.clone());
    desc.field("Мастер", or_none(s.master_name.clone()));
    desc.field("Клиент", or_none(s.party_name.clone()));
    desc.field("Работник клиента", or_none(s.contact_name.clone()));
    desc.field("Машина", or_none(s.vehicle_plate.clone()));
    desc.field("Комментарий", or_none(Some(s.comment.clone())));
    desc.field("Итого", format_som(s.total_tyiyn));
    // Оплата: по способу (и счёту у безнала) — сумма.
    let mut pays: Vec<(String, i64)> = Vec::new();
    for p in &s.payments {
        let label = match &p.account_name {
            Some(a) if matches!(p.method.as_str(), "card" | "transfer") => {
                format!("Оплата: {} ({a})", method_text(&p.method))
            }
            _ => format!("Оплата: {}", method_text(&p.method)),
        };
        match pays.iter_mut().find(|(l, _)| *l == label) {
            Some(x) => x.1 = x.1.saturating_add(p.amount_tyiyn),
            None => pays.push((label, p.amount_tyiyn)),
        }
    }
    for (label, amount) in pays {
        desc.field(&label, format_som(amount));
    }
    desc.lines = s
        .lines
        .iter()
        .map(|l| DescLine {
            key: format!(
                "{}{}",
                l.product_id
                    .or(l.service_id)
                    .map(|u| u.to_string())
                    .unwrap_or_default(),
                if l.gift { ":gift" } else { "" }
            ),
            name: if l.gift {
                format!("{} (подарок)", l.name)
            } else {
                l.name.clone()
            },
            parts: vec![
                (
                    "количество".into(),
                    sale_qty_text(&l.kind, l.qty, l.container_ml),
                ),
                ("цена".into(), format_som(l.unit_price_tyiyn)),
                ("сумма".into(), format_som(l.amount_tyiyn)),
            ],
        })
        .collect();
    let draft = json!({
        "sale_type": s.sale_type,
        "cashier_id": s.cashier_id,
        "master_id": s.master_id,
        "party_id": s.party_id,
        "contact_id": h.contact_id,
        "vehicle_id": h.vehicle_id,
        "comment": s.comment,
        "delivery_address": s.delivery_address,
        "business_date": h.business_date,
        "mileage": book.as_ref().and_then(|b| b.mileage_value),
        "mileage_unit": book.as_ref().map(|b| b.mileage_unit.clone()),
        "declined": book.as_ref().map(|b| b.declined.clone()).unwrap_or_default(),
        "lines": s.lines.iter().map(|l| json!({
            "kind": l.kind,
            "gift": l.gift,
            "product_id": l.product_id,
            "service_id": l.service_id,
            "qty": l.qty,
            "unit_price_tyiyn": l.unit_price_tyiyn,
        })).collect::<Vec<_>>(),
        "payments": s.payments.iter().map(|p| {
            // Аккумулятор: вес и цена за кг, при которых сумма та же (ADR-078).
            let per_kg = p.grams.filter(|g| *g > 0).and_then(|g| {
                crate::domain::money::div_round(i128::from(p.amount_tyiyn) * 1000, i128::from(g))
            }).filter(|price| {
                p.grams.and_then(|g| crate::api::batteries::amount_for(g, *price).ok()) == Some(p.amount_tyiyn)
            });
            json!({
                "method": p.method,
                "amount_tyiyn": p.amount_tyiyn,
                "grams": p.grams,
                "price_per_kg_tyiyn": per_kg,
                "account_id": p.account_id,
            })
        }).collect::<Vec<_>>(),
    });
    Ok(Doc {
        number: Some(s.number),
        draft,
        desc,
    })
}

/// Оплаты накладной, проведённые вместе с ней («оплачено сразу», SPEC-29 §4): погашения того же
/// поставщика из той же транзакции, ещё не сторнированные.
async fn paid_with_receipt(
    conn: &mut PgConnection,
    branch_id: Uuid,
    receipt_id: Uuid,
) -> AppResult<Vec<(Uuid, i64, Option<Uuid>)>> {
    let rows = sqlx::query!(
        r#"select l.id, l.amount_tyiyn,
                  (select m.account_id from cash_movements m
                    where m.doc_type = 'repayment' and m.doc_id = l.doc_id and m.kind <> 'bank_fee'
                    order by m.created_at limit 1) as account_id
           from party_ledger l join receipts r on r.id = $2
           where l.branch_id = $1 and l.party_id = r.supplier_id
             and l.kind = 'repayment' and l.doc_type = 'repayment' and l.reversal_of is null
             and l.created_at = r.created_at
             and not exists (select 1 from party_ledger x where x.reversal_of = l.id)
           order by l.id"#,
        branch_id,
        receipt_id
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.id, r.amount_tyiyn, r.account_id))
        .collect())
}

async fn load_receipt_doc(conn: &mut PgConnection, branch_id: Uuid, id: Uuid) -> AppResult<Doc> {
    let r = crate::api::receipts::load_receipt(conn, branch_id, id).await?;
    let h = sqlx::query!(
        r#"select r.paid_account_id,
                  exists (select 1 from party_ledger l where l.doc_type = 'receipt' and l.doc_id = r.id
                            and l.kind = 'debt') as "on_debt!"
           from receipts r where r.id = $1"#,
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    let now = paid_with_receipt(conn, branch_id, id).await?;
    let paid_now: i64 = now.iter().fold(0i64, |acc, x| acc.saturating_add(x.1));
    let now_account = now.iter().find_map(|x| x.2);
    // Цену продажи ставил сам приход — только у таких товаров она в форме (иначе исправление
    // вернуло бы старую цену поверх поставленной позже).
    let priced: Vec<Uuid> = sqlx::query_scalar!(
        r#"select entity_id as "id!" from audit_log
           where branch_id = $1 and action = 'product.prices' and entity_id is not null
             and data ->> 'receipt_id' = $2"#,
        branch_id,
        id.to_string()
    )
    .fetch_all(&mut *conn)
    .await?;
    let paid_from = if h.on_debt {
        now_account
    } else {
        h.paid_account_id
    };
    let mut desc = Desc::default();
    desc.field("Поставщик", or_none(r.supplier_name.clone()));
    desc.field(
        "Накладная поставщика",
        or_none(Some(r.supplier_doc.clone())),
    );
    desc.field(
        "Расчёт",
        if h.on_debt {
            "в долг"
        } else {
            "оплачена сразу"
        },
    );
    desc.field(
        "Откуда оплачено",
        or_none(account_name(conn, paid_from).await?),
    );
    if paid_now > 0 {
        desc.field("Оплачено сразу", format_som(paid_now));
    }
    desc.field("Сумма", format_som(r.total_tyiyn));
    desc.field("Комментарий", or_none(Some(r.comment.clone())));
    desc.lines = r
        .lines
        .iter()
        .map(|l| {
            let mut parts = vec![
                ("количество".into(), qty_text(l.qty, &l.unit)),
                ("сумма".into(), format_som(l.cost_tyiyn)),
            ];
            if priced.contains(&l.product_id)
                && let Some(p) = l.sale_price_tyiyn
            {
                parts.push(("цена продажи".into(), format_som(p)));
            }
            DescLine {
                key: l.product_id.to_string(),
                name: l.product_name.clone(),
                parts,
            }
        })
        .collect();
    let draft = json!({
        "supplier_id": r.supplier_id,
        "payment": if h.on_debt { "debt" } else { "paid" },
        "supplier_doc": r.supplier_doc,
        "comment": r.comment,
        "paid_from": paid_from.map(|a| a.to_string()),
        "paid_now_tyiyn": (paid_now > 0).then_some(paid_now),
        "lines": r.lines.iter().map(|l| json!({
            "product_id": l.product_id,
            "qty": l.qty,
            "cost_tyiyn": l.cost_tyiyn,
            "sale_price_tyiyn": if priced.contains(&l.product_id) { l.sale_price_tyiyn } else { None },
        })).collect::<Vec<_>>(),
    });
    Ok(Doc {
        number: Some(r.number),
        draft,
        desc,
    })
}

async fn load_ledger_doc(conn: &mut PgConnection, branch_id: Uuid, id: Uuid) -> AppResult<Doc> {
    let l = sqlx::query!(
        r#"select l.party_id, l.kind, l.amount_tyiyn, l.doc_type, l.doc_id, l.comment,
                  p.name, p.role
           from party_ledger l join parties p on p.id = l.party_id
           where l.id = $1 and l.branch_id = $2"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    let mut desc = Desc::default();
    let supplier = l.role == "supplier";
    if l.kind == "adjust" {
        desc.field("Вид", "правка долга");
        desc.field("Контрагент", l.name.clone());
        desc.field(
            "Сумма",
            crate::domain::money::format_som_signed(l.amount_tyiyn),
        );
        desc.field("Комментарий", or_none(Some(l.comment.clone())));
        return Ok(Doc {
            number: None,
            draft: json!({
                "type": "adjust",
                "party_id": l.party_id,
                "amount_tyiyn": l.amount_tyiyn,
                "comment": l.comment,
            }),
            desc,
        });
    }
    // Погашение: способ и накладная — из журнала проведения, счёт — из движения денег.
    let audit = sqlx::query_scalar!(
        r#"select data from audit_log
           where branch_id = $1 and action = 'debt.repayment' and data ->> 'repayment_id' = $2
           order by created_at limit 1"#,
        branch_id,
        l.doc_id.map(|d| d.to_string()).unwrap_or_default()
    )
    .fetch_optional(&mut *conn)
    .await?;
    let method = audit
        .as_ref()
        .and_then(|d| d.get("method"))
        .and_then(Value::as_str)
        .unwrap_or("cash")
        .to_string();
    let receipt_id = audit
        .as_ref()
        .and_then(|d| d.get("receipt_id"))
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok());
    let account_id = sqlx::query_scalar!(
        r#"select account_id from cash_movements
           where doc_type = 'repayment' and doc_id = $1 and kind <> 'bank_fee'
           order by created_at limit 1"#,
        l.doc_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    let receipt_number = match receipt_id {
        Some(r) => {
            sqlx::query_scalar!("select number from receipts where id = $1", r)
                .fetch_optional(&mut *conn)
                .await?
        }
        None => None,
    };
    desc.field(
        "Вид",
        if supplier {
            "оплата поставщику"
        } else {
            "погашение долга"
        },
    );
    desc.field("Контрагент", l.name.clone());
    desc.field("Сумма", format_som(l.amount_tyiyn.abs()));
    desc.field("Способ", method_text(&method));
    desc.field("Счёт", or_none(account_name(conn, account_id).await?));
    desc.field(
        "Накладная",
        or_none(receipt_number.map(|n| format!("№ {n}"))),
    );
    desc.field("Комментарий", or_none(Some(l.comment.clone())));
    Ok(Doc {
        number: None,
        draft: json!({
            "type": "repayment",
            "party_id": l.party_id,
            "amount_tyiyn": l.amount_tyiyn.abs(),
            "method": method,
            "account_id": account_id,
            "receipt_id": receipt_id,
            "comment": l.comment,
        }),
        desc,
    })
}

/// Остатки всех товаров исправления — старого документа и нового — блокируются сразу и по
/// порядку id, как в чеке: иначе сторно держит свои товары, проведение добирает остальные,
/// и встречный чек с теми же товарами ловит взаимную блокировку базы.
async fn prelock_products(
    conn: &mut PgConnection,
    branch_id: Uuid,
    original_id: Uuid,
    doc: &Value,
) -> AppResult<()> {
    fn collect(v: &Value, out: &mut Vec<Uuid>) {
        match v {
            Value::Object(m) => {
                for (k, x) in m {
                    if k == "product_id"
                        && let Some(id) = x.as_str().and_then(|s| Uuid::parse_str(s).ok())
                    {
                        out.push(id);
                    }
                    collect(x, out);
                }
            }
            Value::Array(a) => a.iter().for_each(|x| collect(x, out)),
            _ => {}
        }
    }
    let mut raw: Vec<Uuid> = sqlx::query_scalar!(
        "select distinct product_id from stock_movements where branch_id = $1 and doc_id = $2",
        branch_id,
        original_id
    )
    .fetch_all(&mut *conn)
    .await?;
    collect(doc, &mut raw);
    let mut ids = BTreeSet::new();
    for id in raw {
        let exists = sqlx::query_scalar!("select id from products where id = $1", id)
            .fetch_optional(&mut *conn)
            .await?;
        if exists.is_some() {
            ids.insert(ops::live_product(&mut *conn, id).await?);
        }
    }
    for id in ids {
        ops::lock_pool(&mut *conn, branch_id, id).await?;
    }
    Ok(())
}

/// Можно ли исправлять документ: он проведён, не сторно и не сторнирован, не исправлен. Остальные
/// запреты — у функции сторно (брак по приходу, товар продан, смена закрыта и т. п.).
async fn check_correctable(
    conn: &mut PgConnection,
    branch_id: Uuid,
    kind: Kind,
    id: Uuid,
) -> AppResult<()> {
    check_kind_rules(conn, branch_id, kind, id).await?;
    // Документ проведён мимо касс («из своих» до ADR-079 или накладная, оплаченная до выбора
    // кассы, ADR-062): верный документ без кассы провести уже нельзя, а с кассой — деньги
    // ушли бы оттуда, откуда их не брали. Честный отказ вместо тихой подмены (ADR-088).
    let outside = match kind {
        Kind::Expense => sqlx::query_scalar!(
            r#"select source = 'outside' as "v!" from expenses where id = $1"#,
            id
        )
        .fetch_optional(&mut *conn)
        .await?
        .unwrap_or(false),
        Kind::Payout => sqlx::query_scalar!(
            r#"select source = 'outside' as "v!" from payouts where id = $1"#,
            id
        )
        .fetch_optional(&mut *conn)
        .await?
        .unwrap_or(false),
        Kind::Receipt => sqlx::query_scalar!(
            r#"select coalesce(r.paid_from = 'outside', false)
                      or (r.paid_from is null
                          and not exists (select 1 from party_ledger l
                                          where l.doc_type = 'receipt' and l.doc_id = r.id)) as "v!"
               from receipts r where r.id = $1"#,
            id
        )
        .fetch_optional(&mut *conn)
        .await?
        .unwrap_or(false),
        Kind::Ledger => sqlx::query_scalar!(
            r#"select l.kind = 'repayment' and l.doc_type = 'repayment'
                      and not exists (select 1 from cash_movements m
                                      where m.doc_type = 'repayment' and m.doc_id = l.doc_id) as "v!"
               from party_ledger l where l.id = $1"#,
            id
        )
        .fetch_optional(&mut *conn)
        .await?
        .unwrap_or(false),
        _ => false,
    };
    if outside {
        return Err(AppError::Conflict(
            "документ проведён мимо касс — исправить нельзя: сторнируйте его и проведите верный со счёта".into(),
        ));
    }
    Ok(())
}

/// Запреты исправления по виду документа: сторнирован, сам сторно, чек с возвратом и т. п.
async fn check_kind_rules(
    conn: &mut PgConnection,
    branch_id: Uuid,
    kind: Kind,
    id: Uuid,
) -> AppResult<()> {
    if let Some(n) = sqlx::query_scalar!(
        "select new_number from doc_corrections where original_id = $1",
        id
    )
    .fetch_optional(&mut *conn)
    .await?
    {
        return Err(AppError::Conflict(match n {
            Some(n) => format!("документ уже исправлен — исправляйте новый № {n}"),
            None => "документ уже исправлен — исправляйте новый".into(),
        }));
    }
    let reversed =
        AppError::Conflict("документ сторнирован или это само сторно — исправлять нечего".into());
    match kind {
        Kind::Sale => {
            let s = sqlx::query!(
                r#"select kind, exists (select 1 from sales r where r.reversal_of = s.id) as "returned!"
                   from sales s where s.id = $1 and s.branch_id = $2"#,
                id,
                branch_id
            )
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(AppError::NotFound)?;
            if s.kind != "sale" {
                return Err(AppError::Conflict(
                    "возврат не исправляется — исправьте чек".into(),
                ));
            }
            if s.returned {
                return Err(AppError::Conflict(
                    "по чеку уже был возврат: исправляется только чек без возвратов".into(),
                ));
            }
        }
        Kind::Receipt => {
            let r = sqlx::query!(
                r#"select reversal_of, exists (select 1 from receipts x where x.reversal_of = r.id) as "reversed!"
                   from receipts r where r.id = $1 and r.branch_id = $2"#,
                id,
                branch_id
            )
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(AppError::NotFound)?;
            if r.reversal_of.is_some() || r.reversed {
                return Err(reversed);
            }
        }
        Kind::Expense => {
            let e = sqlx::query!(
                r#"select reversal_of, exists (select 1 from expenses x where x.reversal_of = e.id) as "reversed!"
                   from expenses e where e.id = $1 and e.branch_id = $2"#,
                id,
                branch_id
            )
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(AppError::NotFound)?;
            if e.reversal_of.is_some() || e.reversed {
                return Err(reversed);
            }
        }
        Kind::CashMovement => {
            let m = sqlx::query!(
                r#"select kind, doc_type,
                          exists (select 1 from cash_movements x where x.kind = 'reversal'
                                    and x.doc_type = 'cash_movement' and x.doc_id = m.id) as "reversed!"
                   from cash_movements m where m.id = $1 and m.branch_id = $2"#,
                id,
                branch_id
            )
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(AppError::NotFound)?;
            if m.doc_type != "manual" || !matches!(m.kind.as_str(), "cash_in" | "cash_out") {
                return Err(AppError::Conflict(
                    "это движение исправляется своим документом: чеком, расходом или выплатой"
                        .into(),
                ));
            }
            if m.reversed {
                return Err(reversed);
            }
        }
        Kind::CashTransfer => {
            let t = sqlx::query!(
                r#"select reversal_of,
                          exists (select 1 from cash_transfers x where x.reversal_of = t.id) as "reversed!",
                          exists (select 1 from shift_handovers h where h.transfer_id = t.id) as "handover!"
                   from cash_transfers t where t.id = $1 and t.branch_id = $2"#,
                id,
                branch_id
            )
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(AppError::NotFound)?;
            if t.reversal_of.is_some() || t.reversed {
                return Err(reversed);
            }
            if t.handover {
                return Err(AppError::Conflict(
                    "сдача кассы не исправляется: сторнируйте её и сдайте кассу заново".into(),
                ));
            }
        }
        Kind::Ledger => {
            let l = sqlx::query!(
                r#"select kind, doc_type, reversal_of,
                          exists (select 1 from party_ledger x where x.reversal_of = l.id) as "reversed!"
                   from party_ledger l where l.id = $1 and l.branch_id = $2"#,
                id,
                branch_id
            )
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(AppError::NotFound)?;
            if l.reversal_of.is_some() || l.reversed {
                return Err(reversed);
            }
            if !matches!(
                (l.kind.as_str(), l.doc_type.as_str()),
                ("repayment", "repayment") | ("adjust", "adjust")
            ) {
                return Err(AppError::Conflict(
                    "эта запись долга исправляется своим документом: чеком, приходом, возвратом или браком".into(),
                ));
            }
        }
        Kind::Accrual => {
            let a = sqlx::query!(
                r#"select kind, doc_type, reversal_of,
                          exists (select 1 from payroll_accruals x where x.reversal_of = a.id) as "reversed!"
                   from payroll_accruals a where a.id = $1 and a.branch_id = $2"#,
                id,
                branch_id
            )
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(AppError::NotFound)?;
            if a.reversal_of.is_some() || a.reversed {
                return Err(reversed);
            }
            if !matches!(
                (a.kind.as_str(), a.doc_type.as_str()),
                ("bonus" | "penalty", "manual") | ("monthly_salary", "salary")
            ) {
                return Err(AppError::Conflict(
                    "исправляются премия, удержание и оклад; начисление по чеку или смене — своим документом".into(),
                ));
            }
        }
        Kind::Payout => {
            let p = sqlx::query!(
                r#"select reversal_of, exists (select 1 from payouts x where x.reversal_of = p.id) as "reversed!"
                   from payouts p where p.id = $1 and p.branch_id = $2"#,
                id,
                branch_id
            )
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(AppError::NotFound)?;
            if p.reversal_of.is_some() || p.reversed {
                return Err(reversed);
            }
        }
        Kind::StockIn => {
            let s = sqlx::query!(
                r#"select reversal_of, exists (select 1 from stock_ins x where x.reversal_of = s.id) as "reversed!"
                   from stock_ins s where s.id = $1 and s.branch_id = $2"#,
                id,
                branch_id
            )
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(AppError::NotFound)?;
            if s.reversal_of.is_some() || s.reversed {
                return Err(reversed);
            }
        }
        Kind::SupplierReturn => {
            let s = sqlx::query!(
                r#"select reversal_of, exists (select 1 from supplier_returns x where x.reversal_of = s.id) as "reversed!"
                   from supplier_returns s where s.id = $1 and s.branch_id = $2"#,
                id,
                branch_id
            )
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(AppError::NotFound)?;
            if s.reversal_of.is_some() || s.reversed {
                return Err(reversed);
            }
        }
        Kind::BatteryIntake => {
            let reversed_now = sqlx::query_scalar!(
                r#"select exists (select 1 from battery_intake_reversals x where x.intake_id = b.id) as "r!"
                   from battery_intakes b where b.id = $1 and b.branch_id = $2"#,
                id,
                branch_id
            )
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(AppError::NotFound)?;
            if reversed_now {
                return Err(reversed);
            }
        }
        Kind::OilTransfer => {
            let reversed_now = sqlx::query_scalar!(
                r#"select exists (select 1 from oil_transfer_reversals x where x.transfer_id = t.id) as "r!"
                   from oil_transfers t where t.id = $1 and t.branch_id = $2"#,
                id,
                branch_id
            )
            .fetch_optional(&mut *conn)
            .await?
            .ok_or(AppError::NotFound)?;
            if reversed_now {
                return Err(reversed);
            }
        }
    }
    Ok(())
}

/// Сторно исходного документа его же функцией. Возвращает id сторно.
async fn reverse(
    conn: &mut PgConnection,
    ctx: &Ctx,
    kind: Kind,
    id: Uuid,
    reason: &str,
) -> AppResult<Uuid> {
    let comment = reason.to_string();
    Ok(match kind {
        Kind::Sale => {
            // Возврат всего чека теми же способами, в учётный день чека.
            let h = sqlx::query!("select business_date from sales where id = $1", id)
                .fetch_one(&mut *conn)
                .await?;
            let lines = sqlx::query!(
                "select line_no, qty from sale_lines where sale_id = $1 order by line_no",
                id
            )
            .fetch_all(&mut *conn)
            .await?;
            // Каждая оплата — на свой счёт: безнал двух банков не перекладывается между ними.
            let pays = sqlx::query!(
                r#"select method, account_id, sum(amount_tyiyn)::bigint as "amount!" from sale_payments
                   where sale_id = $1 group by method, account_id order by method, account_id"#,
                id
            )
            .fetch_all(&mut *conn)
            .await?;
            let out = crate::api::sales::return_sale_tx(
                conn,
                ctx,
                id,
                crate::api::sales::ReturnReq {
                    op_id: new_id(),
                    comment,
                    lines: lines
                        .into_iter()
                        .map(|l| crate::api::sales::ReturnLineReq {
                            line_no: l.line_no,
                            qty: l.qty,
                        })
                        .collect(),
                    payments: pays
                        .into_iter()
                        .map(|p| crate::api::sales::PaymentReq {
                            method: p.method,
                            amount_tyiyn: p.amount,
                            grams: None,
                            price_per_kg_tyiyn: None,
                            account_id: p.account_id,
                        })
                        .collect(),
                    battery_cash: false,
                    business_date: Some(h.business_date),
                },
            )
            .await?;
            out.id
        }
        Kind::Receipt => {
            // Оплата, проведённая вместе с накладной, сторнируется с ней: новая накладная
            // проведёт свою (иначе деньги ушли бы дважды).
            for (ledger_id, _, _) in paid_with_receipt(conn, ctx.user.branch_id, id).await? {
                crate::api::parties::reverse_ledger_tx(
                    conn,
                    ctx,
                    ledger_id,
                    crate::api::parties::LedgerReverseReq {
                        op_id: new_id(),
                        comment: comment.clone(),
                    },
                )
                .await?;
            }
            crate::api::receipts::reverse_receipt_tx(
                conn,
                ctx,
                id,
                crate::api::receipts::ReverseReq {
                    op_id: new_id(),
                    comment,
                    in_correction: true,
                },
            )
            .await?
            .id
        }
        Kind::Expense => {
            crate::api::expenses::reverse_expense_tx(
                conn,
                ctx,
                id,
                crate::api::expenses::ReverseReq {
                    op_id: new_id(),
                    comment,
                },
            )
            .await?
            .id
        }
        Kind::CashMovement => crate::api::cash::reverse_movement_tx(
            conn,
            ctx,
            id,
            crate::api::cash::ReverseReq {
                op_id: new_id(),
                comment,
            },
        )
        .await?
        .movement_id
        .ok_or_else(|| AppError::Internal("нет сторно движения".into()))?,
        Kind::CashTransfer => {
            let out = crate::api::cash::reverse_transfer_tx(
                conn,
                ctx,
                id,
                crate::api::cash::ReverseReq {
                    op_id: new_id(),
                    comment,
                },
            )
            .await?;
            value_id(&out)?
        }
        Kind::Ledger => crate::api::parties::reverse_ledger_tx(
            conn,
            ctx,
            id,
            crate::api::parties::LedgerReverseReq {
                op_id: new_id(),
                comment,
            },
        )
        .await?
        .ledger_id
        .ok_or_else(|| AppError::Internal("нет сторно записи долга".into()))?,
        Kind::Accrual => crate::api::payroll::reverse_accrual_tx(
            conn,
            ctx,
            id,
            crate::api::payroll::ReverseReq {
                op_id: new_id(),
                comment,
            },
        )
        .await?
        .id
        .ok_or_else(|| AppError::Internal("нет сторно начисления".into()))?,
        Kind::Payout => {
            crate::api::payroll::reverse_payout_tx(
                conn,
                ctx,
                id,
                crate::api::payroll::ReverseReq {
                    op_id: new_id(),
                    comment,
                },
            )
            .await?
            .id
        }
        Kind::StockIn => {
            crate::api::stock_in::reverse_tx(
                conn,
                ctx,
                id,
                crate::api::stock_in::ReverseReq {
                    op_id: new_id(),
                    comment,
                },
            )
            .await?
            .id
        }
        Kind::SupplierReturn => {
            crate::api::supplier_returns::reverse_tx(
                conn,
                ctx,
                id,
                crate::api::supplier_returns::ReverseReq {
                    op_id: new_id(),
                    comment,
                },
            )
            .await?
            .id
        }
        Kind::BatteryIntake => {
            crate::api::batteries::reverse_intake_tx(
                conn,
                ctx,
                id,
                crate::api::batteries::IntakeReverseReq {
                    op_id: new_id(),
                    comment,
                },
            )
            .await?
            .id
        }
        Kind::OilTransfer => {
            crate::api::oil::reverse_transfer_tx(
                conn,
                ctx,
                id,
                crate::api::oil::ReverseReq {
                    op_id: new_id(),
                    comment,
                },
            )
            .await?;
            sqlx::query_scalar!(
                "select id from oil_transfer_reversals where transfer_id = $1",
                id
            )
            .fetch_one(&mut *conn)
            .await?
        }
    })
}

fn value_id(v: &Value) -> AppResult<Uuid> {
    v.get("id")
        .and_then(Value::as_str)
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| AppError::Internal("нет id документа".into()))
}

/// Запрос проведения из исправленного документа формы: `op_id` — новый, внутренний.
fn req_of<T: DeserializeOwned>(doc: &Value) -> AppResult<T> {
    let mut doc = doc.clone();
    let obj = doc
        .as_object_mut()
        .ok_or_else(|| invalid("нет исправленного документа"))?;
    obj.insert("op_id".into(), json!(new_id()));
    serde_json::from_value(doc).map_err(|e| invalid(format!("исправленный документ: {e}")))
}

fn doc_type(draft: &Value) -> &str {
    draft.get("type").and_then(Value::as_str).unwrap_or("")
}

/// Проведение верного документа его же функцией. Возвращает id и номер.
async fn post_new(
    conn: &mut PgConnection,
    ctx: &Ctx,
    kind: Kind,
    doc: &Value,
    original: &Doc,
) -> AppResult<(Uuid, Option<i64>)> {
    Ok(match kind {
        Kind::Sale => {
            let orig_date = original
                .draft
                .get("business_date")
                .and_then(Value::as_str)
                .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
                .ok_or_else(|| AppError::Internal("нет учётного дня чека".into()))?;
            let date = match doc.get("business_date").and_then(Value::as_str) {
                None => orig_date,
                Some(s) => NaiveDate::parse_from_str(s, "%Y-%m-%d")
                    .map_err(|_| invalid("учётный день: дата ГГГГ-ММ-ДД"))?,
            };
            // Учётный день — как у исходного чека или в окне доверия: сегодня и два дня назад.
            if date != orig_date {
                let today = crate::api::fees::today(conn).await?;
                if date > today || date < today - chrono::Duration::days(2) {
                    return Err(invalid(
                        "учётный день — как у исходного чека или за последние два дня",
                    ));
                }
            }
            let mut req: crate::api::sales::SaleReq = req_of(doc)?;
            req.offline = false;
            req.client_time = None;
            req.correction = Some(crate::api::sales::SaleCorrection {
                business_date: date,
            });
            let out = crate::api::sales::post_sale_tx(conn, ctx, req).await?;
            (out.id, Some(out.number))
        }
        Kind::Receipt => {
            let out = crate::api::receipts::post_receipt_tx(conn, ctx, req_of(doc)?).await?;
            (out.id, Some(out.number))
        }
        Kind::Expense => {
            let out = crate::api::expenses::post_expense_tx(conn, ctx, req_of(doc)?).await?;
            (out.id, Some(out.number))
        }
        Kind::CashMovement => {
            let out = crate::api::cash::post_movement_tx(conn, ctx, req_of(doc)?).await?;
            (
                out.movement_id
                    .ok_or_else(|| AppError::Internal("нет движения".into()))?,
                None,
            )
        }
        Kind::CashTransfer => {
            let out = crate::api::cash::post_transfer_tx(conn, ctx, req_of(doc)?).await?;
            (value_id(&out)?, out.get("number").and_then(Value::as_i64))
        }
        Kind::Ledger => {
            let out = if doc_type(&original.draft) == "adjust" {
                crate::api::parties::post_adjust_tx(conn, ctx, req_of(doc)?).await?
            } else {
                crate::api::parties::post_repayment_tx(conn, ctx, req_of(doc)?).await?
            };
            (
                out.ledger_id
                    .ok_or_else(|| AppError::Internal("нет записи долга".into()))?,
                None,
            )
        }
        Kind::Accrual => {
            let out = if doc_type(&original.draft) == "salary" {
                crate::api::payroll::post_salary_tx(conn, ctx, req_of(doc)?).await?
            } else {
                crate::api::payroll::post_accrual_tx(conn, ctx, req_of(doc)?).await?
            };
            (out.id.ok_or_else(|| invalid("сумма больше нуля"))?, None)
        }
        Kind::Payout => {
            let out = crate::api::payroll::post_payout_tx(conn, ctx, req_of(doc)?).await?;
            (out.id, Some(out.number))
        }
        Kind::StockIn => {
            let out = crate::api::stock_in::post_tx(conn, ctx, req_of(doc)?).await?;
            (out.id, Some(out.number))
        }
        Kind::SupplierReturn => {
            let out = crate::api::supplier_returns::post_tx(conn, ctx, req_of(doc)?).await?;
            (out.id, Some(out.number))
        }
        Kind::BatteryIntake => {
            let out = crate::api::batteries::intake_tx(conn, ctx, req_of(doc)?).await?;
            (out.id, Some(out.number))
        }
        Kind::OilTransfer => {
            let out = crate::api::oil::post_transfer_tx(conn, ctx, req_of(doc)?).await?;
            (out.id, Some(out.number))
        }
    })
}

#[derive(Deserialize)]
pub struct CorrectionReq {
    pub op_id: Uuid,
    pub kind: String,
    pub original_id: Uuid,
    #[serde(default)]
    pub comment: String,
    /// Исправленный документ — в виде запроса его проведения (как черновик), без `op_id`.
    pub doc: Value,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CorrectionOut {
    pub id: Uuid,
    pub kind: String,
    pub kind_label: String,
    pub original_id: Uuid,
    pub original_number: Option<i64>,
    pub reversal_id: Uuid,
    pub new_id: Uuid,
    pub new_number: Option<i64>,
    pub comment: String,
    pub diff: Vec<DiffItem>,
    pub user_name: String,
    pub created_at: DateTime<Utc>,
}

async fn post(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<CorrectionReq>,
) -> AppResult<Json<CorrectionOut>> {
    let mut tx = state.pool.begin().await?;
    let out = correct_tx(&mut tx, &ctx, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Исправление одной транзакцией вызывающего: сторно исходного, проведение верного, связь,
/// журнал. Ошибка любого шага — ничего не проведено.
pub async fn correct_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    req: CorrectionReq,
) -> AppResult<CorrectionOut> {
    const KIND: &str = "correction.post";
    if !ctx.user.is_owner() {
        return Err(AppError::ForbiddenWith(
            "исправляет документы только владелец".into(),
        ));
    }
    let comment = req.comment.trim().to_string();
    if comment.is_empty() {
        return Err(invalid("укажите, что и почему исправляете"));
    }
    if comment.chars().count() > 500 {
        return Err(invalid("комментарий — не длиннее 500 знаков"));
    }
    let kind = Kind::parse(&req.kind)?;
    if let Some(done) = ops::begin_op(&mut *conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let branch_id = ctx.user.branch_id;
    // Два исправления одного документа одновременно: второе ждёт и видит, что он уже исправлен.
    sqlx::query!(
        "select pg_advisory_xact_lock(hashtextextended($1::text, 3))",
        req.original_id.to_string()
    )
    .execute(&mut *conn)
    .await?;
    check_correctable(&mut *conn, branch_id, kind, req.original_id).await?;
    let before = load_doc(&mut *conn, &ctx.user, kind, req.original_id).await?;
    prelock_products(&mut *conn, branch_id, req.original_id, &req.doc).await?;
    // Остатки товаров накладной до исправления: минус после верного прихода — повод для
    // уведомления, временный минус между сторно и верным приходом — нет (ADR-092).
    let stock_before = if kind == Kind::Receipt {
        crate::api::receipts::receipt_stock(&mut *conn, branch_id, req.original_id).await?
    } else {
        Vec::new()
    };
    let reversal_id = reverse(
        &mut *conn,
        ctx,
        kind,
        req.original_id,
        &format!("исправление: {comment}"),
    )
    .await?;
    let (new_doc_id, new_number) = post_new(&mut *conn, ctx, kind, &req.doc, &before).await?;
    if kind == Kind::Receipt {
        let mut lines = Vec::new();
        for (id, name, unit, was) in &stock_before {
            let now = sqlx::query_scalar!(
                r#"select coalesce((select stock_qty from branch_products
                                    where branch_id = $1 and product_id = $2), 0) as "n!""#,
                branch_id,
                id
            )
            .fetch_one(&mut *conn)
            .await?;
            if now < 0 && now < *was {
                lines.push(json!({ "name": name, "unit": unit, "stock": was, "after": now }));
            }
        }
        if !lines.is_empty() {
            ops::audit(
                &mut *conn,
                ctx,
                "receipt.reverse_negative",
                "receipt",
                Some(reversal_id),
                json!({
                    "original": req.original_id,
                    "number": before.number,
                    "new_number": new_number,
                    "correction": true,
                    "lines": lines,
                }),
            )
            .await?;
        }
    }
    let after = load_doc(&mut *conn, &ctx.user, kind, new_doc_id).await?;
    let changes = diff(&before.desc, &after.desc);
    let id = new_id();
    sqlx::query!(
        r#"insert into doc_corrections (id, branch_id, op_id, kind, original_id, original_number,
                                        reversal_id, new_id, new_number, comment, diff, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)"#,
        id,
        branch_id,
        req.op_id,
        kind.as_str(),
        req.original_id,
        before.number,
        reversal_id,
        new_doc_id,
        new_number,
        comment,
        json!(changes),
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await
    .map_err(|e| {
        if crate::error::is_unique_violation(&e) {
            AppError::Conflict("документ уже исправлен".into())
        } else {
            AppError::from(e)
        }
    })?;
    ops::audit(
        &mut *conn,
        ctx,
        KIND,
        kind.as_str(),
        Some(new_doc_id),
        json!({
            "kind": kind.as_str(),
            "label": kind.label(),
            "original_id": req.original_id,
            "original_number": before.number,
            "new_number": new_number,
            "comment": comment,
            "diff": changes,
        }),
    )
    .await?;
    let out = load_correction(&mut *conn, &ctx.user, Some(id), None, None)
        .await?
        .ok_or_else(|| AppError::Internal("исправление не найдено".into()))?;
    ops::finish_op(&mut *conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

/// Исправление по id, по исходному или по новому документу. Администратору — без сравнения:
/// в нём закупочные цены и деньги, которых он не видит.
async fn load_correction(
    conn: &mut PgConnection,
    user: &CurrentUser,
    id: Option<Uuid>,
    original_id: Option<Uuid>,
    new_id: Option<Uuid>,
) -> AppResult<Option<CorrectionOut>> {
    Ok(
        load_corrections(conn, user, id, original_id, new_id, None, 1)
            .await?
            .into_iter()
            .next(),
    )
}

/// Исправления филиала одним запросом, новые сверху: сначала последние `limit` по отбору, из
/// них администратору — без исправлений личных расходов владельца.
async fn load_corrections(
    conn: &mut PgConnection,
    user: &CurrentUser,
    id: Option<Uuid>,
    original_id: Option<Uuid>,
    new_id: Option<Uuid>,
    kind: Option<&str>,
    limit: i64,
) -> AppResult<Vec<CorrectionOut>> {
    let rows = sqlx::query!(
        r#"select c.id, c.kind, c.original_id, c.original_number, c.reversal_id, c.new_id, c.new_number,
                  c.comment, c.diff, u.full_name as user_name, c.created_at
           from (select * from doc_corrections d
                 where d.branch_id = $1
                   and ($2::uuid is null or d.id = $2)
                   and ($3::uuid is null or d.original_id = $3)
                   and ($4::uuid is null or d.new_id = $4)
                   and ($6::text is null or d.kind = $6)
                 order by d.created_at desc, d.id desc
                 limit $7) c
           join users u on u.id = c.user_id
           -- Исправления личных расходов владельца администратору не видны, как и сами
           -- расходы (SPEC-06, ADR-088).
           where $5 or not (c.kind = 'expense' and exists (
                   select 1 from expenses e join expense_articles a on a.id = e.article_id
                   where e.id = c.original_id and a.owner_only))
           order by c.created_at desc, c.id desc"#,
        user.branch_id,
        id,
        original_id,
        new_id,
        user.is_owner(),
        kind,
        limit
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let diff: Vec<DiffItem> = if user.is_owner() {
            serde_json::from_value(r.diff).map_err(|e| AppError::Internal(e.to_string()))?
        } else {
            Vec::new()
        };
        let label = Kind::parse(&r.kind).map(Kind::label).unwrap_or("документ");
        out.push(CorrectionOut {
            id: r.id,
            kind: r.kind,
            kind_label: label.into(),
            original_id: r.original_id,
            original_number: r.original_number,
            reversal_id: r.reversal_id,
            new_id: r.new_id,
            new_number: r.new_number,
            comment: r.comment,
            diff,
            user_name: r.user_name,
            created_at: r.created_at,
        });
    }
    Ok(out)
}

#[derive(Serialize)]
struct DraftOut {
    kind: String,
    kind_label: String,
    original_id: Uuid,
    number: Option<i64>,
    doc: Value,
}

/// Исходный документ для формы «Исправление документа № N». Только владелец; документ, который
/// нельзя исправлять, — сразу 409 с причиной.
async fn draft(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((kind, id)): Path<(String, Uuid)>,
) -> AppResult<Json<DraftOut>> {
    if !user.is_owner() {
        return Err(AppError::ForbiddenWith(
            "исправляет документы только владелец".into(),
        ));
    }
    let kind = Kind::parse(&kind)?;
    let mut conn = state.pool.acquire().await?;
    check_correctable(&mut conn, user.branch_id, kind, id).await?;
    // Запреты сторно (брак по приходу, товар уже продан, нужна открытая смена…) — сразу, до
    // формы: сторно пробуется в транзакции, которая всегда откатывается (SPEC-29 §24).
    {
        let mut tx = state.pool.begin().await?;
        let probe = Ctx {
            user: user.clone(),
            device_id: Uuid::nil(),
            bot_chat: None,
        };
        let tried = reverse(&mut tx, &probe, kind, id, "проверка исправления").await;
        tx.rollback().await?;
        tried?;
    }
    let doc = load_doc(&mut conn, &user, kind, id).await?;
    Ok(Json(DraftOut {
        kind: kind.as_str().into(),
        kind_label: kind.label().into(),
        original_id: id,
        number: doc.number,
        doc: doc.draft,
    }))
}

#[derive(Serialize)]
struct LinksOut {
    /// Этот документ исправлен: на какой.
    corrected_by: Option<CorrectionOut>,
    /// Этот документ — исправление: какого.
    correction_of: Option<CorrectionOut>,
}

async fn links(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<LinksOut>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(LinksOut {
        corrected_by: load_correction(&mut conn, &user, None, Some(id), None).await?,
        correction_of: load_correction(&mut conn, &user, None, None, Some(id)).await?,
    }))
}

#[derive(Deserialize)]
struct ListQuery {
    kind: Option<String>,
}

/// Исправления филиала (вида `kind`) — списки помечают ими строки «исправлен → № N» и
/// «исправление № M», не спрашивая по каждой.
async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    axum::extract::Query(q): axum::extract::Query<ListQuery>,
) -> AppResult<Json<Vec<CorrectionOut>>> {
    let kind = q.kind.as_deref().map(Kind::parse).transpose()?;
    let mut conn = state.pool.acquire().await?;
    Ok(Json(
        load_corrections(
            &mut conn,
            &user,
            None,
            None,
            None,
            kind.map(Kind::as_str),
            500,
        )
        .await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    type Line<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)]);

    fn d(fields: &[(&str, &str)], lines: &[Line<'_>]) -> Desc {
        Desc {
            fields: fields
                .iter()
                .map(|(l, v)| ((*l).to_string(), (*v).to_string()))
                .collect(),
            lines: lines
                .iter()
                .map(|(k, n, parts)| DescLine {
                    key: (*k).to_string(),
                    name: (*n).to_string(),
                    parts: parts
                        .iter()
                        .map(|(l, v)| ((*l).to_string(), (*v).to_string()))
                        .collect(),
                })
                .collect(),
        }
    }

    #[test]
    fn diff_shows_changed_fields_and_lines() {
        let a = d(
            &[("Сумма", "100 с"), ("Комментарий", "—")],
            &[
                ("p1", "Масло", &[("количество", "1 шт"), ("цена", "100 с")]),
                ("p2", "Фильтр", &[("количество", "1 шт")]),
            ],
        );
        let b = d(
            &[
                ("Сумма", "150 с"),
                ("Комментарий", "—"),
                ("Оплата: карта", "150 с"),
            ],
            &[
                ("p1", "Масло", &[("количество", "2 шт"), ("цена", "100 с")]),
                ("p3", "Свеча", &[("количество", "1 шт")]),
            ],
        );
        let out = diff(&a, &b);
        let what: Vec<&str> = out.iter().map(|x| x.what.as_str()).collect();
        assert_eq!(
            what,
            vec![
                "Сумма",
                "Оплата: карта",
                "Масло: количество",
                "Фильтр: строка",
                "Свеча: строка"
            ]
        );
        assert_eq!(out[0].old, "100 с");
        assert_eq!(out[1].old, "—");
        assert_eq!(out[3].new, "убрана");
        assert_eq!(out[4].old, "не было");
    }

    #[test]
    fn same_doc_has_no_diff_and_kinds_round_trip() {
        let a = d(
            &[("Сумма", "1 с")],
            &[("p", "Т", &[("количество", "1 шт")])],
        );
        assert!(diff(&a, &a).is_empty());
        for k in Kind::ALL {
            assert_eq!(Kind::parse(k.as_str()).ok(), Some(k));
        }
        assert!(Kind::parse("revision").is_err());
    }
}
