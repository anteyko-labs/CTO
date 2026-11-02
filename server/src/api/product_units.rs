//! Единица и объём канистры товара (задача 002, П4; задача 003, Н10; ADR-139).
//!
//! Меняет только владелец. Без остатка — сразу. С остатком — одной операцией в каждом филиале:
//! списание старым остатком и приход новым по той же стоимости (I1, I3, I6, I7), движения
//! `unit_change`. Остаток, который не делится нацело, не пересчитывается — 422 с подсказкой.
//! Прошлые документы остаются в прежних единицах; `product_unit_changes` переводит их единицы
//! в нынешние (возврат по старому чеку).
//!
//! Килограммы и метры (ADR-174, ADR-175): штуки ↔ кг/м — с коэффициентом «1 шт = X г/мм» и с
//! остатком; без коэффициента и между разными размерностями (кг ↔ м, кг ↔ л, м ↔ л) — только
//! без остатка, такая смена помечается `reset`: количества прошлых документов в нынешние единицы
//! не переводятся (возврат по ним — 409).

use axum::extract::{Path, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::catalog::{ProductOut, price_unit, product_by_id};
use crate::auth::Ctx;
use crate::domain::money::div_round;
use crate::domain::qty::stock_text;
use crate::error::{AppError, AppResult, invalid, overflow};
use crate::ops::{self, Movement, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/products/{id}/unit", routing::post(change_unit))
}

/// Новая единица товара.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct UnitReq {
    pub op_id: Uuid,
    /// `ml` — литрами (канистра и розлив), `piece` — штуками, `g` — килограммами (ADR-174),
    /// `mm` — метрами (ADR-175).
    pub unit: String,
    /// Объём канистры, мл — для литров.
    #[serde(default)]
    pub container_ml: Option<i64>,
    /// «1 шт = N мл»: литры → штуки и штуки → литры (без него — одна штука = канистра).
    #[serde(default)]
    pub piece_ml: Option<i64>,
    /// Литры → литры с другой канистрой: `count` — канистр столько же (по умолчанию),
    /// `volume` — литров столько же.
    #[serde(default)]
    pub keep: Option<String>,
    /// «1 шт = X г» или «1 шт = X мм» (тысячные кг/м): штуки → кг/м и кг/м → штуки с остатком.
    /// Без него смена между штуками и кг/м — только без остатка (ADR-175).
    #[serde(default)]
    pub piece_qty: Option<i64>,
}

/// Остаток филиала до и после.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct BranchRecount {
    pub branch_id: Uuid,
    pub branch_name: String,
    pub from_qty: i64,
    pub to_qty: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct UnitOut {
    pub product: ProductOut,
    pub branches: Vec<BranchRecount>,
}

const LIMIT_ML: i64 = 1_000_000;

fn gcd(a: i64, b: i64) -> i64 {
    let (mut a, mut b) = (a.abs(), b.abs());
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

/// `qty × mul / div` целым числом; не делится — `None`.
pub fn exact(qty: i64, mul: i64, div: i64) -> Option<i64> {
    if div <= 0 {
        return None;
    }
    let n = i128::from(qty) * i128::from(mul);
    (n % i128::from(div) == 0)
        .then(|| i64::try_from(n / i128::from(div)).ok())
        .flatten()
}

/// Количество в новой единице с одним округлением (I2) — для выключенного правила подарка, у
/// которого значение не переходит целым числом.
fn rounded(qty: i64, mul: i64, div: i64) -> AppResult<i64> {
    div_round(i128::from(qty) * i128::from(mul), i128::from(div)).ok_or_else(overflow)
}

/// Дробь `a / b` с перемножением и сокращением: цена за единицу и «последняя закупочная»
/// хранятся парой (стоимость, количество).
fn reduce(a: i128, b: i128) -> AppResult<(i64, i64)> {
    let g = {
        let (mut x, mut y) = (a.abs(), b.abs());
        while y != 0 {
            (x, y) = (y, x % y);
        }
        x.max(1)
    };
    Ok((
        i64::try_from(a / g).map_err(|_| overflow())?,
        i64::try_from(b / g).map_err(|_| overflow())?,
    ))
}

/// Как пересчитать: новая единица, объём, множитель и делитель остатка, множитель и делитель цены.
/// `reset` — смена без коэффициента между разными единицами (ADR-175): только без остатка,
/// количества прошлых документов не переводятся, цена — то же число за новую единицу цены
/// (канистра — за её литры).
struct Plan {
    unit: &'static str,
    container_ml: Option<i64>,
    mul: i64,
    div: i64,
    price_mul: i64,
    price_div: i64,
    reset: bool,
}

/// Смена без коэффициента: остаток не переводится, цена — `× price_mul / price_div`.
fn reset_plan(
    unit: &'static str,
    container_ml: Option<i64>,
    price_mul: i64,
    price_div: i64,
) -> Plan {
    Plan {
        unit,
        container_ml,
        mul: 1,
        div: 1,
        price_mul,
        price_div,
        reset: true,
    }
}

/// «1 шт = X г/мм»: от 0,001 до 1000 кг (м).
fn check_piece_qty(v: i64, unit: &str) -> AppResult<i64> {
    if (1..=LIMIT_ML).contains(&v) {
        return Ok(v);
    }
    Err(invalid(match unit {
        "mm" => "сколько метров в одной штуке — от 0,001 до 1000 м",
        _ => "сколько килограммов в одной штуке — от 0,001 до 1000 кг",
    }))
}

fn check_ml(v: Option<i64>, what: &str) -> AppResult<i64> {
    match v {
        Some(v) if (1..=LIMIT_ML).contains(&v) => Ok(v),
        Some(v) if v > LIMIT_ML => Err(invalid(format!("{what} — не больше 1000 л"))),
        _ => Err(invalid(format!("укажите {what}"))),
    }
}

fn plan(from: &str, from_c: Option<i64>, req: &UnitReq) -> AppResult<Plan> {
    match (from, req.unit.as_str(), from_c) {
        ("ml", "piece", Some(c)) => {
            // Литры → штуки: остаток в мл делится на «1 шт = N мл»; цена канистры → цена штуки.
            let n = check_ml(req.piece_ml, "сколько мл в одной штуке")?;
            Ok(Plan {
                unit: "piece",
                container_ml: None,
                mul: 1,
                div: n,
                price_mul: n,
                price_div: c,
                reset: false,
            })
        }
        ("piece", "ml", _) => {
            // Штуки → литры: штука — N мл (по умолчанию — канистра); цена штуки → цена канистры.
            let c = check_ml(req.container_ml, "объём канистры")?;
            let n = match req.piece_ml {
                Some(_) => check_ml(req.piece_ml, "сколько мл в одной штуке")?,
                None => c,
            };
            Ok(Plan {
                unit: "ml",
                container_ml: Some(c),
                mul: n,
                div: 1,
                price_mul: c,
                price_div: n,
                reset: false,
            })
        }
        ("ml", "ml", Some(old)) => {
            let c = check_ml(req.container_ml, "объём канистры")?;
            if c == old {
                return Err(invalid("объём канистры тот же — менять нечего"));
            }
            let (mul, div) = match req.keep.as_deref() {
                None | Some("count") => {
                    let g = gcd(c, old);
                    (c / g, old / g)
                }
                Some("volume") => (1, 1),
                Some(_) => return Err(invalid("остаток: count — канистры, volume — литры")),
            };
            Ok(Plan {
                unit: "ml",
                container_ml: Some(c),
                mul,
                div,
                price_mul: 1,
                price_div: 1,
                reset: false,
            })
        }
        ("piece", "piece", _) => Err(invalid("товар уже в штуках — менять нечего")),
        ("g", "g", _) => Err(invalid("товар уже в килограммах — менять нечего")),
        ("mm", "mm", _) => Err(invalid("товар уже в метрах — менять нечего")),
        // Штуки ↔ кг/м с коэффициентом «1 шт = X г/мм» — как штуки ↔ литры (ADR-175): остаток
        // × X или ÷ X целым, цена штуки ↔ цена кг/м через X.
        ("piece", to @ ("g" | "mm"), _) if req.piece_qty.is_some() => {
            let x = check_piece_qty(req.piece_qty.unwrap_or_default(), to)?;
            Ok(Plan {
                unit: if to == "g" { "g" } else { "mm" },
                container_ml: None,
                mul: x,
                div: 1,
                price_mul: 1000,
                price_div: x,
                reset: false,
            })
        }
        (from @ ("g" | "mm"), "piece", _) if req.piece_qty.is_some() => {
            let x = check_piece_qty(req.piece_qty.unwrap_or_default(), from)?;
            Ok(Plan {
                unit: "piece",
                container_ml: None,
                mul: 1,
                div: x,
                price_mul: x,
                price_div: 1000,
                reset: false,
            })
        }
        // Без коэффициента и между разными размерностями — только без остатка (`reset`,
        // ADR-174, ADR-175). Цена — то же число за новую единицу цены; цена канистры — за её
        // литры (× 1000 / объём), и обратно.
        ("piece", "g", _) => Ok(reset_plan("g", None, 1, 1)),
        ("piece", "mm", _) => Ok(reset_plan("mm", None, 1, 1)),
        ("g" | "mm", "piece", _) => Ok(reset_plan("piece", None, 1, 1)),
        ("ml", "g", Some(c)) => Ok(reset_plan("g", None, 1000, c)),
        ("ml", "mm", Some(c)) => Ok(reset_plan("mm", None, 1000, c)),
        ("g", "mm", _) => Ok(reset_plan("mm", None, 1, 1)),
        ("mm", "g", _) => Ok(reset_plan("g", None, 1, 1)),
        ("g" | "mm", "ml", _) => {
            let c = check_ml(req.container_ml, "объём канистры")?;
            Ok(reset_plan("ml", Some(c), c, 1000))
        }
        _ => Err(invalid(
            "единица товара — литры, штуки, килограммы или метры",
        )),
    }
}

/// Что мешает пересчёту: незаконченные документы с количеством в прежних единицах.
async fn blockers(conn: &mut PgConnection, id: Uuid, category_id: Uuid) -> AppResult<()> {
    let revision = sqlx::query!(
        r#"select r.number, b.name from revisions r join branches b on b.id = r.branch_id
           where r.status = 'draft'
             and (r.category_id is null or r.category_id = $2
                  or exists (select 1 from revision_lines l where l.revision_id = r.id and l.product_id = $1))
           order by r.created_at limit 1"#,
        id,
        category_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(r) = revision {
        return Err(AppError::Conflict(format!(
            "идёт ревизия № {} (филиал «{}»): проведите или отмените её, потом меняйте единицу",
            r.number, r.name
        )));
    }
    let waiting = sqlx::query_scalar!(
        r#"select exists (select 1 from receipt_acceptance_lines l
                          join receipt_acceptances a on a.id = l.acceptance_id
                          where a.status = 'waiting' and l.product_id = $1) as "e!""#,
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    if waiting {
        return Err(AppError::Conflict(
            "товар ждёт приёмки на планшете: примите или уберите приёмку, потом меняйте единицу"
                .into(),
        ));
    }
    let defect = sqlx::query_scalar!(
        "select number from defects where product_id = $1 and status = 'pending' order by number limit 1",
        id
    )
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(n) = defect {
        return Err(AppError::Conflict(format!(
            "по товару есть брак, ждущий решения (№ {n}): сначала решите его"
        )));
    }
    let cost = sqlx::query_scalar!(
        r#"select exists (select 1 from payroll_cost_pending
                          where product_id = $1 and resolved_at is null) as "e!""#,
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    if cost {
        return Err(AppError::Conflict(
            "товар продан без цены закупки: сначала укажите цену закупки".into(),
        ));
    }
    Ok(())
}

async fn change_unit(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<UnitReq>,
) -> AppResult<Json<UnitOut>> {
    let mut tx = state.pool.begin().await?;
    let out = change_unit_tx(&mut tx, &ctx, id, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Пересчёт единицы — одна транзакция (I7), повтор `op_id` — тот же ответ (I8).
pub async fn change_unit_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    req: UnitReq,
) -> AppResult<UnitOut> {
    const KIND: &str = "product.unit";
    // Единица и объём канистры меняют остаток всех филиалов — только владелец (I12, I14).
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if let Some(done) = ops::begin_op(&mut *conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let cur = sqlx::query!(
        r#"select name, unit, container_ml, category_id, merged_into, sale_price_tyiyn,
                  points_mode, points_value, system_role
           from products where id = $1 for update"#,
        id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    if cur.merged_into.is_some() {
        return Err(invalid(
            "карточка влита в другую: меняйте единицу у основной",
        ));
    }
    // Карточка лома аккумуляторов — всегда в граммах (ADR-048, ADR-174).
    if cur.system_role.is_some() {
        return Err(invalid(
            "это служебная карточка лома аккумуляторов: её единица не меняется",
        ));
    }
    let p = plan(&cur.unit, cur.container_ml, &req)?;
    blockers(&mut *conn, id, cur.category_id).await?;
    // Подарком товар в кг или метрах быть не может (ADR-175): «1 г в подарок» — бессмыслица.
    if matches!(p.unit, "g" | "mm") {
        let rule = sqlx::query_scalar!(
            r#"select t.name from gift_rule_items i
               join gift_rules r on r.id = i.rule_id
               join products t on t.id = r.trigger_product_id
               where i.gift_product_id = $1 order by t.name limit 1"#,
            id
        )
        .fetch_optional(&mut *conn)
        .await?;
        if let Some(t) = rule {
            return Err(invalid(format!(
                "товар — подарок в правиле «за {t}»: уберите его из подарков, потом переводите \
                 в {}",
                unit_into(p.unit, None)
            )));
        }
    }
    let rows = sqlx::query!(
        r#"select bp.branch_id, b.name as branch_name, bp.stock_qty, bp.stock_value_tyiyn, bp.min_stock,
                  bp.sale_price_tyiyn, bp.last_cost_qty, bp.last_cost_tyiyn,
                  bp.owner_cost_qty, bp.owner_cost_tyiyn
           from branch_products bp join branches b on b.id = bp.branch_id
           where bp.product_id = $1 order by b.name, b.id for update of bp"#,
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    // Остаток каждого филиала должен перейти целым числом — иначе ничего не меняем.
    let mut recount = Vec::new();
    // Без коэффициента разные единицы не пересчитываются друг в друга: только без остатка во
    // всех филиалах (ADR-174, ADR-175).
    if p.reset {
        for r in &rows {
            if r.stock_qty == 0 {
                continue;
            }
            let hint = if cur.unit == "piece" || p.unit == "piece" {
                " — или укажите, сколько в одной штуке"
            } else {
                ""
            };
            return Err(invalid(format!(
                "остаток в филиале «{}» — {}: {} и {} друг в друга не пересчитываются. \
                 Обнулите остаток ревизией, потом меняйте единицу{hint}",
                r.branch_name,
                stock_text(r.stock_qty, &cur.unit, cur.container_ml),
                unit_plural(&cur.unit),
                unit_plural(p.unit),
            )));
        }
    }
    for r in &rows {
        let to = exact(r.stock_qty, p.mul, p.div).ok_or_else(|| {
            let stock = stock_text(r.stock_qty, &cur.unit, cur.container_ml);
            invalid(match p.unit {
                "piece" if matches!(cur.unit.as_str(), "g" | "mm") => format!(
                    "остаток в филиале «{}» — {stock}: это не целое число штук по {}. Продайте \
                     или спишите ревизией остаток до целого или укажите другое количество в штуке",
                    r.branch_name,
                    crate::domain::qty::qty_text(p.div, &cur.unit)
                ),
                "piece" => format!(
                    "остаток в филиале «{}» — {stock}: это не целое число штук по {}. Продайте \
                     или спишите ревизией остаток до целого или выберите другой объём штуки",
                    r.branch_name,
                    crate::domain::qty::liters(p.div)
                ),
                _ => format!(
                    "остаток в филиале «{}» — {stock}: с новой канистрой он не переходит в целые \
                     миллилитры. Выберите «литров столько же» или другой объём",
                    r.branch_name
                ),
            })
        })?;
        recount.push(BranchRecount {
            branch_id: r.branch_id,
            branch_name: r.branch_name.clone(),
            from_qty: r.stock_qty,
            to_qty: to,
        });
    }
    // Подарок по правилу — в единицах учёта (SPEC-11): переводится так же, целым числом.
    // Выключенное правило пересчёт не держит: количество округляется (не меньше одного), правило
    // помечается «поправьте порог» и остаётся выключенным (задача 003, хвост ADR-139).
    let gifts = sqlx::query!(
        r#"select i.rule_id, i.gift_qty, r.active
           from gift_rule_items i join gift_rules r on r.id = i.rule_id
           where i.gift_product_id = $1 for update"#,
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut gift_log = Vec::new();
    let mut needs_fix: Vec<Uuid> = Vec::new();
    for g in &gifts {
        // Смена без коэффициента количество подарка не переводит: включённое правило — отказ,
        // выключенное — «поправьте» (ADR-149, ADR-175).
        if p.reset {
            if g.active {
                return Err(invalid(
                    "товар — подарок во включённом правиле: выключите правило подарка или \
                     уберите товар из подарков, потом меняйте единицу",
                ));
            }
            needs_fix.push(g.rule_id);
            continue;
        }
        let qty = match exact(g.gift_qty, p.mul, p.div).filter(|q| *q > 0) {
            Some(q) => q,
            None if !g.active => {
                needs_fix.push(g.rule_id);
                rounded(g.gift_qty, p.mul, p.div)?.max(1)
            }
            None => {
                return Err(invalid(
                    "товар — подарок в правиле, и его количество не переходит целым числом: \
                     поправьте правило подарка",
                ));
            }
        };
        sqlx::query!(
            "update gift_rule_items set gift_qty = $3 where rule_id = $1 and gift_product_id = $2",
            g.rule_id,
            id,
            qty
        )
        .execute(&mut *conn)
        .await?;
        gift_log.push(json!({ "rule_id": g.rule_id, "from": g.gift_qty, "to": qty }));
    }
    // Порог правила, где товар — условие («от 8 л»), — в единицах учёта товара-условия
    // (SPEC-11): переводится так же, целым числом, иначе ничего не меняем.
    // Выключенное правило — так же, как подарок: округляется и помечается.
    let thresholds = sqlx::query!(
        "select id, min_units, active from gift_rules where trigger_product_id = $1 for update",
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut threshold_log = Vec::new();
    for t in &thresholds {
        // Порог в прежней единице без коэффициента не переводится (ADR-175).
        if p.reset {
            if t.active {
                return Err(invalid(format!(
                    "товар — условие включённого правила подарка «от {}»: выключите правило \
                     подарка, потом меняйте единицу, и поправьте порог",
                    stock_text(t.min_units, &cur.unit, cur.container_ml)
                )));
            }
            needs_fix.push(t.id);
            continue;
        }
        let to = match exact(t.min_units, p.mul, p.div) {
            Some(to) => to,
            None if !t.active => {
                needs_fix.push(t.id);
                rounded(t.min_units, p.mul, p.div)?
            }
            None => {
                return Err(invalid(format!(
                    "товар — условие правила подарка «от {}»: порог не переходит в новую единицу \
                     целым числом: поправьте порог правила подарка, потом меняйте единицу",
                    stock_text(t.min_units, &cur.unit, cur.container_ml)
                )));
            }
        };
        if to == t.min_units {
            continue;
        }
        sqlx::query!(
            "update gift_rules set min_units = $2 where id = $1",
            t.id,
            to
        )
        .execute(&mut *conn)
        .await?;
        threshold_log.push(json!({ "rule_id": t.id, "from": t.min_units, "to": to }));
    }
    needs_fix.sort();
    needs_fix.dedup();
    if !needs_fix.is_empty() {
        sqlx::query!(
            "update gift_rules set needs_fix = true where id = any($1)",
            &needs_fix
        )
        .execute(&mut *conn)
        .await?;
    }
    // Баллы «за единицу» товара (ADR-140) — ставка за штуку, литр, кг или метр: в новую единицу
    // переводится обратно количеству, чтобы за тот же товар начислялось столько же; одно
    // округление `div_round` (I2), было → стало — в журнал. Смена без коэффициента — то же число
    // за новую единицу (ADR-175).
    let points = if cur.points_mode == "fixed" && cur.points_value > 0 && !p.reset {
        let to = points_rate(cur.points_value, &cur.unit, p.unit, p.mul, p.div)?;
        if to <= 0 {
            return Err(invalid(format!(
                "баллы товара — {} за {}: в новой единице это меньше 0,01 с. Поправьте баллы \
                 товара, потом меняйте единицу",
                crate::domain::money::format_som(cur.points_value),
                unit_word(&cur.unit)
            )));
        }
        sqlx::query!(
            "update products set points_value = $2 where id = $1",
            id,
            to
        )
        .execute(&mut *conn)
        .await?;
        Some(json!({ "from": cur.points_value, "to": to }))
    } else {
        None
    };
    let price = |v: i64| {
        div_round(
            i128::from(v) * i128::from(p.price_mul),
            i128::from(p.price_div),
        )
        .ok_or_else(overflow)
    };
    let shared_price = price(cur.sale_price_tyiyn)?;
    sqlx::query!(
        "update products set unit = $2, container_ml = $3, sale_price_tyiyn = $4 where id = $1",
        id,
        p.unit,
        p.container_ml,
        shared_price
    )
    .execute(&mut *conn)
    .await?;
    let change_id = new_id();
    sqlx::query!(
        r#"insert into product_unit_changes (id, product_id, from_unit, from_container_ml, to_unit,
                                             to_container_ml, qty_mul, qty_div, user_id, device_id,
                                             reset)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)"#,
        change_id,
        id,
        cur.unit,
        cur.container_ml,
        p.unit,
        p.container_ml,
        p.mul,
        p.div,
        ctx.user.id,
        ctx.device_id,
        p.reset
    )
    .execute(&mut *conn)
    .await?;
    // Цена за единицу — парой (стоимость, количество): новая единица = старая × div / mul.
    // Без коэффициента — как цена продажи: за единицу цены то же число × price_mul / price_div
    // (единицы цены у прежней и новой единицы свои: канистра, кг, м, штука).
    let (cost_mul, cost_div) = if p.reset {
        (
            i128::from(price_unit(&cur.unit, cur.container_ml)) * i128::from(p.price_mul),
            i128::from(price_unit(p.unit, p.container_ml)) * i128::from(p.price_div),
        )
    } else {
        (i128::from(p.div), i128::from(p.mul))
    };
    let per = |cost: i64, qty: i64| -> AppResult<(i64, i64)> {
        if qty <= 0 || cost <= 0 {
            return Ok((cost, qty));
        }
        let (c, q) = reduce(i128::from(cost) * cost_mul, i128::from(qty) * cost_div)?;
        Ok((c, q))
    };
    let mut prices = Vec::new();
    for (r, rc) in rows.iter().zip(&recount) {
        let (last_cost, last_qty) = per(r.last_cost_tyiyn, r.last_cost_qty)?;
        let (owner_cost, owner_qty) = per(r.owner_cost_tyiyn, r.owner_cost_qty)?;
        // Минимальный остаток без коэффициента не переводится — обнуляется (ADR-175).
        let min_stock = if p.reset {
            0
        } else {
            div_round(
                i128::from(r.min_stock) * i128::from(p.mul),
                i128::from(p.div),
            )
            .ok_or_else(overflow)?
        };
        let sale = price(r.sale_price_tyiyn)?;
        prices.push(json!({ "branch": r.branch_name, "from": r.sale_price_tyiyn, "to": sale }));
        sqlx::query!(
            r#"update branch_products set min_stock = $3, sale_price_tyiyn = $4,
                      last_cost_qty = $5, last_cost_tyiyn = $6, owner_cost_qty = $7, owner_cost_tyiyn = $8
               where branch_id = $1 and product_id = $2"#,
            r.branch_id,
            id,
            min_stock,
            sale,
            last_qty,
            last_cost,
            owner_qty,
            owner_cost
        )
        .execute(&mut *conn)
        .await?;
        if rc.from_qty == rc.to_qty {
            continue;
        }
        // Списание старым остатком и приход новым — по той же стоимости (I3, I6).
        ops::apply_movement(
            &mut *conn,
            Movement {
                branch_id: r.branch_id,
                product_id: id,
                qty_delta: -rc.from_qty,
                value_delta: -r.stock_value_tyiyn,
                doc_type: "unit_change",
                doc_id: change_id,
            },
        )
        .await?;
        ops::apply_movement(
            &mut *conn,
            Movement {
                branch_id: r.branch_id,
                product_id: id,
                qty_delta: rc.to_qty,
                value_delta: r.stock_value_tyiyn,
                doc_type: "unit_change",
                doc_id: change_id,
            },
        )
        .await?;
    }
    ops::audit(
        &mut *conn,
        ctx,
        "product.unit_change",
        "product",
        Some(id),
        json!({
            "name": cur.name,
            "from": { "unit": cur.unit, "container_ml": cur.container_ml },
            "to": { "unit": p.unit, "container_ml": p.container_ml },
            "qty_mul": p.mul,
            "qty_div": p.div,
            "reset": p.reset,
            "branches": recount.iter().map(|r| json!({
                "branch": r.branch_name, "from_qty": r.from_qty, "to_qty": r.to_qty,
            })).collect::<Vec<_>>(),
            "prices": prices,
            "gift_qty": gift_log,
            "gift_thresholds": threshold_log,
            // Выключенные правила, у которых значение округлено: «поправьте порог».
            "gift_rules_need_fix": needs_fix,
            "points": points,
        }),
    )
    .await?;
    let product = product_by_id(&mut *conn, &ctx.user, id).await?;
    let out = UnitOut {
        product,
        branches: recount,
    };
    ops::finish_op(&mut *conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

/// Сколько единиц учёта в единице правила «за единицу» (`loyalty::line_rules`): литр — 1000 мл,
/// кг — 1000 г, метр — 1000 мм, штука — 1 (единица цены без канистры).
fn per_unit(unit: &str) -> i64 {
    price_unit(unit, None)
}

fn unit_word(unit: &str) -> &'static str {
    match unit {
        "ml" => "литр",
        "g" => "кг",
        "mm" => "метр",
        _ => "штуку",
    }
}

/// «штуки», «литры», «килограммы», «метры» — в тексте отказа.
fn unit_plural(unit: &str) -> &'static str {
    match unit {
        "ml" => "литры",
        "g" => "килограммы",
        "mm" => "метры",
        _ => "штуки",
    }
}

/// Ставка баллов «за единицу» в новой единице: количество переходит `× mul / div`, ставка за
/// единицу учёта — `× div / mul`, и единица правила (литр или штука) — своя у каждой единицы.
pub fn points_rate(value: i64, from: &str, to: &str, mul: i64, div: i64) -> AppResult<i64> {
    div_round(
        i128::from(value) * i128::from(div) * i128::from(per_unit(to)),
        i128::from(mul) * i128::from(per_unit(from)),
    )
    .ok_or_else(overflow)
}

/// Количество в единицах документа от `since` — в нынешних единицах товара: все пересчёты
/// после документа по порядку. Не переходит целым числом — 409.
pub async fn units_now(
    conn: &mut PgConnection,
    product_id: Uuid,
    since: DateTime<Utc>,
    qty: i64,
) -> AppResult<i64> {
    let changes = sqlx::query!(
        r#"select qty_mul, qty_div, reset from product_unit_changes
           where product_id = $1 and created_at > $2 order by created_at, id"#,
        product_id,
        since
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut q = qty;
    for c in changes {
        // Смена без коэффициента (кг ↔ шт, кг ↔ м и т. п., ADR-175): прежние количества в
        // нынешние единицы не переводятся.
        if c.reset {
            return Err(AppError::Conflict(
                "после документа товар переведён в другую единицу без пересчёта остатка: \
                 количество документа не переходит в новую единицу, поправьте остаток ревизией"
                    .into(),
            ));
        }
        q = exact(q, c.qty_mul, c.qty_div).ok_or_else(|| {
            AppError::Conflict(
                "после документа единица товара изменилась, и количество не переходит целым \
                 числом: поправьте остаток ревизией"
                    .into(),
            )
        })?;
    }
    Ok(q)
}

/// После документа `doc_id` единица товара менялась — сторно документа не проводится: его
/// количества в прежних единицах (ADR-139). Поправка — ревизией. Время документа — его первое
/// движение склада.
pub async fn forbid_after_doc(
    conn: &mut PgConnection,
    product_id: Uuid,
    doc_id: Uuid,
) -> AppResult<()> {
    let name = sqlx::query_scalar!(
        r#"select p.name from products p
           where p.id = $1
             and exists (select 1 from product_unit_changes c
                         where c.product_id = p.id
                           and c.created_at > (select min(m.created_at) from stock_movements m
                                               where m.doc_id = $2))"#,
        product_id,
        doc_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    match name {
        Some(n) => Err(AppError::Conflict(format!(
            "после документа у товара «{n}» изменилась единица или объём канистры: \
             отменить документ нельзя, поправьте остаток ревизией"
        ))),
        None => Ok(()),
    }
}

/// Как товар учитывается после пересчёта: «литрах (канистра 5 л)» или «штуках».
fn unit_words(unit: &str, container_ml: Option<i64>) -> String {
    match (unit, container_ml) {
        ("ml", Some(c)) => format!("литрах (канистра {})", crate::domain::qty::liters(c)),
        ("ml", None) => "литрах".into(),
        ("g", _) => "килограммах".into(),
        ("mm", _) => "метрах".into(),
        _ => "штуках".into(),
    }
}

/// «в штуки», «в литры (канистра 4 л)».
fn unit_into(unit: &str, container_ml: Option<i64>) -> String {
    match (unit, container_ml) {
        ("ml", Some(c)) => format!("литры (канистра {})", crate::domain::qty::liters(c)),
        ("ml", None) => "литры".into(),
        ("g", _) => "килограммы".into(),
        ("mm", _) => "метры".into(),
        _ => "штуки".into(),
    }
}

/// Строка документа, открытого до пересчёта единицы (ADR-139, дополнено): форма прислала
/// единицу и объём канистры, какими видела товар; вызывается после блокировки товара
/// (`receipts::lock_products`) — товар в другой единице, 409 «обновите строку». Пусто — не
/// сверяется (прежний клиент).
pub async fn ensure_seen_unit(
    conn: &mut PgConnection,
    product_id: Uuid,
    unit: Option<&str>,
    container_ml: Option<i64>,
) -> AppResult<()> {
    let Some(unit) = unit else {
        return Ok(());
    };
    let p = sqlx::query!(
        "select name, unit, container_ml from products where id = $1",
        product_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| invalid("товар не найден"))?;
    let same = p.unit == unit && (p.unit != "ml" || p.container_ml == container_ml);
    if same {
        return Ok(());
    }
    Err(stale_line(&p.name, &p.unit, p.container_ml))
}

/// «Товар «…» только что переведён в штуки, обновите строку» — 409.
pub fn stale_line(name: &str, unit: &str, container_ml: Option<i64>) -> AppError {
    AppError::Conflict(format!(
        "Товар «{name}» только что переведён в {}, обновите строку",
        unit_into(unit, container_ml)
    ))
}

/// Строка чека в другой единице, чем товар (штуки у товара в метрах, «на вес» у метрового —
/// касса не знала метров), — 409 «обновите строку» (ADR-175).
pub fn wrong_kind(name: &str, unit: &str, container_ml: Option<i64>) -> AppError {
    AppError::Conflict(format!(
        "Товар «{name}» учитывается в {}, обновите строку",
        unit_words(unit, container_ml)
    ))
}

/// Строка чека с видом прежней единицы (канистра у товара, который перевели в штуки): после
/// пересчёта единицы — 409 «обновите строку» вместо «вид строки не подходит товару».
pub async fn changed_from(
    conn: &mut PgConnection,
    product_id: Uuid,
    unit: &str,
) -> AppResult<bool> {
    Ok(sqlx::query_scalar!(
        r#"select exists (select 1 from product_unit_changes
                          where product_id = $1 and from_unit = $2) as "e!""#,
        product_id,
        unit
    )
    .fetch_one(&mut *conn)
    .await?)
}

/// «Изменить оплату» и «Исправить» чека повторяют его строки в прежних единицах: если после
/// чека у товара менялись единица или объём канистры, новый чек не сложится. Понятный отказ
/// 422 вместо «вид строки не подходит товару» (ADR-139, задача 003): чек исправляют возвратом
/// (он переводит количество в нынешние единицы) и новым чеком.
pub async fn forbid_sale_replay(conn: &mut PgConnection, sale_id: Uuid) -> AppResult<()> {
    let hit = sqlx::query!(
        r#"select p.name, c.to_unit, c.to_container_ml,
                  (c.created_at at time zone 'Asia/Bishkek')::date as "day!"
           from sales s
           join sale_lines l on l.sale_id = s.id
           join products p on p.id = l.product_id
           join product_unit_changes c on c.product_id = l.product_id and c.created_at > s.created_at
           where s.id = $1
           order by l.line_no, c.created_at desc, c.id desc
           limit 1"#,
        sale_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    match hit {
        Some(h) => Err(AppError::Validation(format!(
            "Товар «{}» с {:02}.{:02}.{} учитывается в {}; исправьте чек через возврат и новый чек",
            h.name,
            h.day.day(),
            h.day.month(),
            h.day.year(),
            unit_words(&h.to_unit, h.to_container_ml)
        ))),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(
        unit: &str,
        container_ml: Option<i64>,
        piece_ml: Option<i64>,
        keep: Option<&str>,
    ) -> UnitReq {
        UnitReq {
            op_id: Uuid::nil(),
            unit: unit.into(),
            container_ml,
            piece_ml,
            keep: keep.map(Into::into),
            piece_qty: None,
        }
    }

    fn by_piece(unit: &str, piece_qty: i64) -> UnitReq {
        UnitReq {
            piece_qty: Some(piece_qty),
            ..req(unit, None, None, None)
        }
    }

    #[test]
    fn exact_only_whole() {
        assert_eq!(exact(4_000, 1, 1_000), Some(4));
        assert_eq!(exact(4_500, 1, 1_000), None);
        assert_eq!(exact(-2_000, 1, 1_000), Some(-2));
        assert_eq!(exact(3, 4_000, 1), Some(12_000));
    }

    #[test]
    fn plans() {
        let p = plan("ml", Some(4_000), &req("piece", None, Some(4_000), None)).unwrap();
        assert_eq!(
            (p.mul, p.div, p.price_mul, p.price_div),
            (1, 4_000, 4_000, 4_000)
        );
        let p = plan("piece", None, &req("ml", Some(1_000), None, None)).unwrap();
        assert_eq!((p.mul, p.div, p.container_ml), (1_000, 1, Some(1_000)));
        // 4 л → 5 л, канистр столько же: 12 л → 15 л.
        let p = plan("ml", Some(4_000), &req("ml", Some(5_000), None, None)).unwrap();
        assert_eq!(exact(12_000, p.mul, p.div), Some(15_000));
        let p = plan(
            "ml",
            Some(4_000),
            &req("ml", Some(5_000), None, Some("volume")),
        )
        .unwrap();
        assert_eq!((p.mul, p.div), (1, 1));
        assert!(plan("ml", Some(4_000), &req("ml", Some(4_000), None, None)).is_err());
        // Килограммы и метры без коэффициента (ADR-174, ADR-175) — только без остатка, `reset`;
        // цена канистры 4 л → за кг (м).
        let p = plan("g", None, &req("piece", None, None, None)).unwrap();
        assert_eq!((p.unit, p.reset), ("piece", true));
        let p = plan("piece", None, &req("g", None, None, None)).unwrap();
        assert_eq!((p.unit, p.reset), ("g", true));
        let p = plan("ml", Some(4_000), &req("g", None, None, None)).unwrap();
        assert_eq!((p.price_mul, p.price_div, p.reset), (1_000, 4_000, true));
        let p = plan("ml", Some(4_000), &req("mm", None, None, None)).unwrap();
        assert_eq!((p.unit, p.price_mul, p.price_div), ("mm", 1_000, 4_000));
        let p = plan("g", None, &req("mm", None, None, None)).unwrap();
        assert_eq!((p.unit, p.reset), ("mm", true));
        let p = plan("mm", None, &req("ml", Some(5_000), None, None)).unwrap();
        assert_eq!(
            (p.container_ml, p.price_mul, p.price_div),
            (Some(5_000), 5_000, 1_000)
        );
        assert!(plan("g", None, &req("ml", None, None, None)).is_err());
        assert!(plan("g", None, &req("g", None, None, None)).is_err());
        assert!(plan("mm", None, &req("mm", None, None, None)).is_err());
        assert!(plan("ml", Some(4_000), &req("piece", None, None, None)).is_err());
        // С коэффициентом «1 шт = 2,5 м»: 4 шт → 10 000 мм, цена штуки 500 с → 200 с за м.
        let p = plan("piece", None, &by_piece("mm", 2_500)).unwrap();
        assert_eq!((p.unit, p.mul, p.div, p.reset), ("mm", 2_500, 1, false));
        assert_eq!(exact(4, p.mul, p.div), Some(10_000));
        assert_eq!(
            div_round(50_000 * i128::from(p.price_mul), i128::from(p.price_div)),
            Some(20_000)
        );
        // «1 шт = 0,5 кг»: 1 500 г → 3 шт; 1 250 г — не целое.
        let p = plan("g", None, &by_piece("piece", 500)).unwrap();
        assert_eq!(
            (p.mul, p.div, p.price_mul, p.price_div),
            (1, 500, 500, 1_000)
        );
        assert_eq!(exact(1_500, p.mul, p.div), Some(3));
        assert_eq!(exact(1_250, p.mul, p.div), None);
        assert!(plan("piece", None, &by_piece("g", 0)).is_err());
        assert!(plan("piece", None, &by_piece("g", 1_000_001)).is_err());
    }

    #[test]
    fn points_rate_for_meters() {
        // 5 с за штуку, «1 шт = 2 м» → 2,5 с за метр.
        assert_eq!(points_rate(500, "piece", "mm", 2_000, 1).unwrap(), 250);
        assert_eq!(per_unit("mm"), 1_000);
        assert_eq!(unit_word("mm"), "метр");
        assert_eq!(unit_words("mm", None), "метрах");
        assert_eq!(unit_into("g", None), "килограммы");
    }

    #[test]
    fn reduce_keeps_ratio() {
        // 2000 с за 4000 мл → за штуку по 1 л: 500 с за 1 шт.
        let (c, q) = reduce(200_000 * 1_000, 4_000).unwrap();
        assert_eq!((c, q), (50_000, 1));
    }
}
