//! Единица и объём канистры товара (задача 002, П4; задача 003, Н10; ADR-139).
//!
//! Меняет только владелец. Без остатка — сразу. С остатком — одной операцией в каждом филиале:
//! списание старым остатком и приход новым по той же стоимости (I1, I3, I6, I7), движения
//! `unit_change`. Остаток, который не делится нацело, не пересчитывается — 422 с подсказкой.
//! Прошлые документы остаются в прежних единицах; `product_unit_changes` переводит их единицы
//! в нынешние (возврат по старому чеку).

use axum::extract::{Path, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Datelike, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::catalog::{ProductOut, product_by_id};
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
    /// `ml` — литрами (канистра и розлив), `piece` — штуками, `g` — килограммами (ADR-174).
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
struct Plan {
    unit: &'static str,
    container_ml: Option<i64>,
    mul: i64,
    div: i64,
    price_mul: i64,
    price_div: i64,
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
            })
        }
        ("piece", "piece", _) => Err(invalid("товар уже в штуках — менять нечего")),
        ("g", "g", _) => Err(invalid("товар уже в килограммах — менять нечего")),
        // Килограммы (ADR-174) — только без остатка (`zero_only`): штука и литр не равны
        // килограмму, пересчитывать нечем. Цена переходит как «1 шт = 1 кг», «1 л = 1 кг»:
        // цена штуки — цена кг, цена канистры — за её литры; владелец поправит её в карточке.
        ("piece", "g", _) => Ok(Plan {
            unit: "g",
            container_ml: None,
            mul: 1000,
            div: 1,
            price_mul: 1,
            price_div: 1,
        }),
        ("ml", "g", Some(c)) => Ok(Plan {
            unit: "g",
            container_ml: None,
            mul: 1,
            div: 1,
            price_mul: 1000,
            price_div: c,
        }),
        ("g", "piece", _) => Ok(Plan {
            unit: "piece",
            container_ml: None,
            mul: 1,
            div: 1000,
            price_mul: 1,
            price_div: 1,
        }),
        ("g", "ml", _) => {
            let c = check_ml(req.container_ml, "объём канистры")?;
            Ok(Plan {
                unit: "ml",
                container_ml: Some(c),
                mul: 1,
                div: 1,
                price_mul: c,
                price_div: 1000,
            })
        }
        _ => Err(invalid("единица товара — литры, штуки или килограммы")),
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
    // Килограммы и штуки (литры) не пересчитываются друг в друга: только без остатка (ADR-174).
    let zero_only = (cur.unit == "g") != (p.unit == "g");
    for r in &rows {
        if zero_only && r.stock_qty != 0 {
            return Err(invalid(format!(
                "остаток в филиале «{}» — {}: килограммы и {} друг в друга не пересчитываются. \
                 Обнулите остаток ревизией, потом меняйте единицу",
                r.branch_name,
                stock_text(r.stock_qty, &cur.unit, cur.container_ml),
                if cur.unit == "piece" || p.unit == "piece" {
                    "штуки"
                } else {
                    "литры"
                }
            )));
        }
    }
    for r in &rows {
        let to = exact(r.stock_qty, p.mul, p.div).ok_or_else(|| {
            let stock = stock_text(r.stock_qty, &cur.unit, cur.container_ml);
            invalid(match p.unit {
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
    // Баллы «за единицу» товара (ADR-140) — ставка за штуку, литр или кг: в новую единицу
    // переводится обратно количеству, чтобы за тот же товар начислялось столько же; одно
    // округление `div_round` (I2), было → стало — в журнал.
    let points = if cur.points_mode == "fixed" && cur.points_value > 0 {
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
                                             to_container_ml, qty_mul, qty_div, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"#,
        change_id,
        id,
        cur.unit,
        cur.container_ml,
        p.unit,
        p.container_ml,
        p.mul,
        p.div,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await?;
    // Цена за единицу — парой (стоимость, количество): новая единица = старая × div / mul.
    let per = |cost: i64, qty: i64| -> AppResult<(i64, i64)> {
        if qty <= 0 || cost <= 0 {
            return Ok((cost, qty));
        }
        let (c, q) = reduce(
            i128::from(cost) * i128::from(p.div),
            i128::from(qty) * i128::from(p.mul),
        )?;
        Ok((c, q))
    };
    let mut prices = Vec::new();
    for (r, rc) in rows.iter().zip(&recount) {
        let (last_cost, last_qty) = per(r.last_cost_tyiyn, r.last_cost_qty)?;
        let (owner_cost, owner_qty) = per(r.owner_cost_tyiyn, r.owner_cost_qty)?;
        let min_stock = div_round(
            i128::from(r.min_stock) * i128::from(p.mul),
            i128::from(p.div),
        )
        .ok_or_else(overflow)?;
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
/// кг — 1000 г, штука — 1.
fn per_unit(unit: &str) -> i64 {
    match unit {
        "ml" | "g" => 1000,
        _ => 1,
    }
}

fn unit_word(unit: &str) -> &'static str {
    match unit {
        "ml" => "литр",
        "g" => "кг",
        _ => "штуку",
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
        r#"select qty_mul, qty_div from product_unit_changes
           where product_id = $1 and created_at > $2 order by created_at, id"#,
        product_id,
        since
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut q = qty;
    for c in changes {
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
        _ => "штуках".into(),
    }
}

/// «в штуки», «в литры (канистра 4 л)».
fn unit_into(unit: &str, container_ml: Option<i64>) -> String {
    match (unit, container_ml) {
        ("ml", Some(c)) => format!("литры (канистра {})", crate::domain::qty::liters(c)),
        ("ml", None) => "литры".into(),
        ("g", _) => "килограммы".into(),
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
        // Килограммы (ADR-174): «1 шт = 1 кг», «1 л = 1 кг»; цена канистры 4 л → за кг.
        let p = plan("g", None, &req("piece", None, None, None)).unwrap();
        assert_eq!((p.unit, p.mul, p.div), ("piece", 1, 1_000));
        let p = plan("piece", None, &req("g", None, None, None)).unwrap();
        assert_eq!((p.unit, p.mul, p.div), ("g", 1_000, 1));
        let p = plan("ml", Some(4_000), &req("g", None, None, None)).unwrap();
        assert_eq!((p.price_mul, p.price_div), (1_000, 4_000));
        assert!(plan("g", None, &req("ml", None, None, None)).is_err());
        assert!(plan("g", None, &req("g", None, None, None)).is_err());
        assert!(plan("ml", Some(4_000), &req("piece", None, None, None)).is_err());
    }

    #[test]
    fn reduce_keeps_ratio() {
        // 2000 с за 4000 мл → за штуку по 1 л: 500 с за 1 шт.
        let (c, q) = reduce(200_000 * 1_000, 4_000).unwrap();
        assert_eq!((c, q), (50_000, 1));
    }
}
