//! Бонусные баллы клиентов, подключённых к Телеграм-боту (SPEC-19, ADR-053).
//! 1 балл = 1 сом; начисление — по строкам чека с оплаченного деньгами: правило товара →
//! категории → общая ставка, по умолчанию только за чек с заменой масла (ADR-124, ADR-140);
//! списание — оплата чека баллами.

use axum::extract::{Path, Query, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::sales::PaymentReq;
use crate::auth::{Ctx, CurrentUser};
use crate::domain::money::{div_round, format_som};
use crate::domain::points;
use crate::error::{AppError, AppResult, invalid, overflow};
use crate::ops::{self, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/loyalty/lookup", routing::get(lookup))
        .route("/parties/{id}/loyalty", routing::get(party_history))
        .route(
            "/settings/loyalty",
            routing::get(get_settings).put(put_settings),
        )
}

/// Процент начисления по умолчанию — 0,5 % (решение заказчика).
const DEFAULT_RATE_BP: i32 = 50;

/// Настройка баллов филиала: общая ставка и «только в чеке с заменой масла» (ADR-140).
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub rate_bp: i32,
    /// По умолчанию — да, как было (ADR-124): чек без замены масла баллов не даёт.
    #[serde(default = "yes")]
    pub oil_change_only: bool,
}

fn yes() -> bool {
    true
}

pub async fn settings(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<Settings> {
    let v = sqlx::query_scalar!(
        "select value from settings where branch_id = $1 and key = 'loyalty'",
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    let rate_bp = v
        .as_ref()
        .and_then(|v| v.get("rate_bp").and_then(serde_json::Value::as_i64))
        .and_then(|d| i32::try_from(d).ok())
        .filter(|d| (0..=1000).contains(d))
        .unwrap_or(DEFAULT_RATE_BP);
    let oil_change_only = v
        .as_ref()
        .and_then(|v| {
            v.get("oil_change_only")
                .and_then(serde_json::Value::as_bool)
        })
        .unwrap_or(true);
    Ok(Settings {
        rate_bp,
        oil_change_only,
    })
}

pub async fn rate_bp(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<i32> {
    Ok(settings(conn, branch_id).await?.rate_bp)
}

pub async fn balance(conn: &mut PgConnection, party_id: Uuid) -> AppResult<i64> {
    Ok(sqlx::query_scalar!(
        r#"select coalesce(sum(amount_tyiyn), 0)::bigint as "v!" from loyalty_ledger where party_id = $1"#,
        party_id
    )
    .fetch_one(&mut *conn)
    .await?)
}

/// Баллы у всех клиентов (SPEC-30 §12; клиенты и баллы общие для филиалов — SPEC-33): сколько клиентов с баллами и сколько баллов
/// у них вместе. 1 балл = 1 сом, баллы хранятся в тыйынах, как деньги.
#[derive(Serialize, Deserialize, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PointsTotal {
    pub clients: i64,
    pub points_tyiyn: i64,
}

pub async fn branch_total(conn: &mut PgConnection) -> AppResult<PointsTotal> {
    let r = sqlx::query!(
        r#"select count(*) filter (where b > 0) as "clients!",
                  coalesce(sum(b) filter (where b > 0), 0)::bigint as "points!"
           from (select party_id, sum(amount_tyiyn) as b from loyalty_ledger
                 group by party_id) x"#
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(PointsTotal {
        clients: r.clients,
        points_tyiyn: r.points,
    })
}

/// Участник программы — клиент, подключившийся к боту своим номером.
pub async fn is_member(conn: &mut PgConnection, party_id: Uuid) -> AppResult<bool> {
    Ok(sqlx::query_scalar!(
        r#"select exists (select 1 from telegram_customers where party_id = $1) as "e!""#,
        party_id
    )
    .fetch_one(&mut *conn)
    .await?)
}

struct Entry<'a> {
    branch_id: Uuid,
    party_id: Uuid,
    kind: &'a str,
    amount: i64,
    rate_bp: Option<i32>,
    doc_type: &'a str,
    doc_id: Uuid,
}

async fn add(conn: &mut PgConnection, e: Entry<'_>) -> AppResult<Option<Uuid>> {
    if e.amount == 0 {
        return Ok(None);
    }
    let id = new_id();
    sqlx::query!(
        r#"insert into loyalty_ledger (id, branch_id, party_id, kind, amount_tyiyn, rate_bp, doc_type, doc_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8)"#,
        id,
        e.branch_id,
        e.party_id,
        e.kind,
        e.amount,
        e.rate_bp,
        e.doc_type,
        e.doc_id
    )
    .execute(&mut *conn)
    .await?;
    Ok(Some(id))
}

/// Из чего сложилась запись начисления или снятия: строки исходного чека (ADR-140).
async fn add_lines(
    conn: &mut PgConnection,
    ledger_id: Uuid,
    sale_id: Uuid,
    rows: &[(i32, i64, &str)],
) -> AppResult<()> {
    let rows: Vec<&(i32, i64, &str)> = rows.iter().filter(|r| r.1 != 0).collect();
    if rows.is_empty() {
        return Ok(());
    }
    let line_nos: Vec<i32> = rows.iter().map(|r| r.0).collect();
    let amounts: Vec<i64> = rows.iter().map(|r| r.1).collect();
    let sources: Vec<String> = rows.iter().map(|r| r.2.to_string()).collect();
    sqlx::query!(
        r#"insert into loyalty_lines (ledger_id, sale_id, line_no, amount_tyiyn, source)
           select $1, $2, x.line_no, x.amount, x.source
           from unnest($3::int[], $4::bigint[], $5::text[]) as x(line_no, amount, source)"#,
        ledger_id,
        sale_id,
        &line_nos,
        &amounts,
        &sources
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn lock_party(conn: &mut PgConnection, party_id: Uuid) -> AppResult<()> {
    sqlx::query!(
        "select pg_advisory_xact_lock(hashtextextended($1::text, 7))",
        party_id.to_string()
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

fn sum_of(payments: &[PaymentReq], pred: impl Fn(&str) -> bool) -> AppResult<i64> {
    payments
        .iter()
        .filter(|p| pred(&p.method))
        .try_fold(0i64, |acc, p| acc.checked_add(p.amount_tyiyn))
        .ok_or_else(overflow)
}

fn is_money(method: &str) -> bool {
    matches!(method, "cash" | "card" | "transfer" | "invoice")
}

/// Строка чека для начисления баллов (ADR-140).
pub struct PointsLine {
    pub line_no: i32,
    pub product_id: Option<Uuid>,
    /// Единицы учёта строки: штуки, мл, граммы.
    pub units: i64,
    /// Сумма строки после скидок; подарок — 0.
    pub amount: i64,
}

/// Правила баллов строк: товар → категория → общая ставка; услуга — общая ставка.
async fn line_rules(
    conn: &mut PgConnection,
    lines: &[PointsLine],
    common_bp: i32,
) -> AppResult<Vec<(points::LineIn, points::Source)>> {
    let ids: Vec<Uuid> = lines.iter().filter_map(|l| l.product_id).collect();
    let rows = sqlx::query!(
        r#"select p.id, p.unit, p.points_mode, p.points_value,
                  c.points_mode as cat_mode, c.points_value as cat_value
           from products p join categories c on c.id = p.category_id
           where p.id = any($1)"#,
        &ids
    )
    .fetch_all(&mut *conn)
    .await?;
    let common = i64::from(common_bp);
    Ok(lines
        .iter()
        .map(|l| {
            let found = l.product_id.and_then(|id| rows.iter().find(|r| r.id == id));
            let (rule, source) = match found {
                Some(r) => points::pick(
                    points::rule_of(&r.points_mode, r.points_value),
                    points::rule_of(&r.cat_mode, r.cat_value),
                    common,
                ),
                None => points::pick(None, None, common),
            };
            let per_unit = match found.map(|r| r.unit.as_str()) {
                Some("ml" | "g") => 1000,
                _ => 1,
            };
            (
                points::LineIn {
                    amount: l.amount,
                    units: l.units,
                    per_unit,
                    rule,
                },
                source,
            )
        })
        .collect())
}

/// Чек: списание баллов (оплата «bonus», в любом чеке) и начисление с оплаченного деньгами —
/// по строкам (ADR-140); по умолчанию только за замену масла (`oil_change`, ADR-124). Долг и
/// баллы не начисляют: начисление — только с реально полученных денег.
pub async fn on_sale(
    conn: &mut PgConnection,
    ctx: &Ctx,
    sale_id: Uuid,
    party_id: Option<Uuid>,
    payments: &[PaymentReq],
    oil_change: bool,
    lines: &[PointsLine],
) -> AppResult<()> {
    let branch_id = ctx.user.branch_id;
    let redeem = sum_of(payments, |m| m == "bonus")?;
    let Some(party_id) = party_id else {
        if redeem > 0 {
            return Err(invalid("для оплаты баллами укажите клиента"));
        }
        return Ok(());
    };
    let member = is_member(conn, party_id).await?;
    if redeem > 0 {
        if !member {
            return Err(invalid("клиент не подключён к боту — баллов у него нет"));
        }
        // Касса без сети баллы не предлагает, но чек с баллами может прийти из очереди: онлайн-
        // отправка оборвалась без ответа, и чек ушёл повторно с признаком «без сети». Продажа
        // уже состоялась — проверяем остаток сейчас, под блокировкой, как у онлайн-чека: хватает —
        // проводим, нет — отказ, и чек уходит владельцу как отклонённый (ADR-059).
        lock_party(conn, party_id).await?;
        let have = balance(conn, party_id).await?;
        if redeem > have {
            return Err(invalid(format!(
                "у клиента {} баллов, списать {} нельзя",
                format_som(have.max(0)),
                format_som(redeem)
            )));
        }
        add(
            conn,
            Entry {
                branch_id,
                party_id,
                kind: "redeem",
                amount: -redeem,
                rate_bp: None,
                doc_type: "sale",
                doc_id: sale_id,
            },
        )
        .await?;
    }
    if !member {
        return Ok(());
    }
    let s = settings(conn, branch_id).await?;
    if s.oil_change_only && !oil_change {
        return Ok(());
    }
    let rules = line_rules(conn, lines, s.rate_bp).await?;
    let money = sum_of(payments, is_money)?;
    let ins: Vec<points::LineIn> = rules.iter().map(|r| r.0).collect();
    let per_line = points::accrue(money, &ins).ok_or_else(overflow)?;
    let accrual = per_line
        .iter()
        .try_fold(0i64, |acc, v| acc.checked_add(*v))
        .ok_or_else(overflow)?;
    let ledger = add(
        conn,
        Entry {
            branch_id,
            party_id,
            kind: "accrual",
            amount: accrual,
            rate_bp: Some(s.rate_bp),
            doc_type: "sale",
            doc_id: sale_id,
        },
    )
    .await?;
    if let Some(ledger_id) = ledger {
        let rows: Vec<(i32, i64, &str)> = lines
            .iter()
            .zip(per_line.iter())
            .zip(rules.iter())
            .map(|((l, v), r)| (l.line_no, *v, r.1.as_str()))
            .collect();
        add_lines(conn, ledger_id, sale_id, &rows).await?;
    }
    Ok(())
}

/// Строка возврата для баллов: сколько возвращают из строки исходного чека и сколько из неё
/// уже вернули раньше.
pub struct ReturnPoints {
    pub line_no: i32,
    pub qty: i64,
    pub orig_qty: i64,
    pub done_qty: i64,
}

/// Возврат: баллы, которыми платили, возвращаются на счёт; возвращённая строка снимает свои
/// начисленные баллы в доле возвращённого (ADR-140), последняя часть — точный остаток. Чек,
/// проведённый до баллов по строкам, — как раньше: с возвращённых денег по той же ставке, не
/// больше начисленного. У чека без начисления снимать нечего.
pub async fn on_return(
    conn: &mut PgConnection,
    ctx: &Ctx,
    orig_id: Uuid,
    return_id: Uuid,
    party_id: Option<Uuid>,
    payments: &[PaymentReq],
    lines: &[ReturnPoints],
) -> AppResult<()> {
    let branch_id = ctx.user.branch_id;
    let back = sum_of(payments, |m| m == "bonus")?;
    let money = sum_of(payments, is_money)?;
    let Some(party_id) = party_id else {
        if back > 0 {
            return Err(invalid("по чеку не платили баллами"));
        }
        return Ok(());
    };
    lock_party(conn, party_id).await?;
    let done = sqlx::query!(
        r#"select coalesce(-sum(l.amount_tyiyn) filter (where l.kind = 'redeem'), 0)::bigint as "redeemed!",
                  coalesce(-sum(l.amount_tyiyn) filter (where l.kind = 'refund'), 0)::bigint as "refunded!",
                  coalesce(sum(l.amount_tyiyn) filter (where l.kind in ('accrual', 'accrual_back')), 0)::bigint as "accrued!",
                  max(l.rate_bp) filter (where l.kind = 'accrual') as rate
           from loyalty_ledger l
           where l.doc_id = $1 or l.doc_id in (select id from sales where reversal_of = $1)"#,
        orig_id
    )
    .fetch_one(&mut *conn)
    .await?;
    let left = done.redeemed + done.refunded;
    if back > left.max(0) {
        return Err(invalid(format!(
            "баллами по этому чеку можно вернуть не больше {}",
            format_som(left.max(0))
        )));
    }
    add(
        conn,
        Entry {
            branch_id,
            party_id,
            kind: "refund",
            amount: back,
            rate_bp: None,
            doc_type: "sale_return",
            doc_id: return_id,
        },
    )
    .await?;
    let Some(rate) = done.rate else {
        return Ok(());
    };
    let room = done.accrued.max(0);
    // Начислено и уже снято по строкам исходного чека, источник правила строки.
    let by_line = sqlx::query!(
        r#"select x.line_no,
                  coalesce(sum(x.amount_tyiyn) filter (where x.amount_tyiyn > 0), 0)::bigint as "accrued!",
                  coalesce(-sum(x.amount_tyiyn) filter (where x.amount_tyiyn < 0), 0)::bigint as "taken!",
                  min(x.source) as "source!"
           from loyalty_lines x where x.sale_id = $1 group by x.line_no"#,
        orig_id
    )
    .fetch_all(&mut *conn)
    .await?;
    if by_line.is_empty() {
        // Чек до баллов по строкам: с возвращённых денег по ставке чека.
        let take = div_round(i128::from(money) * i128::from(rate), 10_000)
            .ok_or_else(overflow)?
            .min(room);
        add(
            conn,
            Entry {
                branch_id,
                party_id,
                kind: "accrual_back",
                amount: -take,
                rate_bp: Some(rate),
                doc_type: "sale_return",
                doc_id: return_id,
            },
        )
        .await?;
        return Ok(());
    }
    let mut rows: Vec<(i32, i64, &str)> = Vec::new();
    let mut total: i64 = 0;
    for l in lines {
        let Some(b) = by_line.iter().find(|b| b.line_no == l.line_no) else {
            continue;
        };
        let rest = (b.accrued - b.taken).max(0);
        let take = if l.done_qty + l.qty >= l.orig_qty {
            rest
        } else {
            div_round(
                i128::from(b.accrued) * i128::from(l.qty),
                i128::from(l.orig_qty),
            )
            .ok_or_else(overflow)?
            .clamp(0, rest)
        }
        .min(room - total);
        if take > 0 {
            total += take;
            rows.push((l.line_no, -take, b.source.as_str()));
        }
    }
    let ledger = add(
        conn,
        Entry {
            branch_id,
            party_id,
            kind: "accrual_back",
            amount: -total,
            rate_bp: Some(rate),
            doc_type: "sale_return",
            doc_id: return_id,
        },
    )
    .await?;
    if let Some(ledger_id) = ledger {
        add_lines(conn, ledger_id, orig_id, &rows).await?;
    }
    Ok(())
}

// ---------- Касса: найти клиента по номеру ----------

#[derive(Deserialize)]
struct LookupQuery {
    phone: String,
}

#[derive(Serialize)]
struct LookupOut {
    party_id: Uuid,
    name: String,
    phone: String,
    balance_tyiyn: i64,
}

async fn lookup(
    State(state): State<AppState>,
    _user: CurrentUser,
    Query(q): Query<LookupQuery>,
) -> AppResult<Json<LookupOut>> {
    let key = crate::api::telegram::phone_key(&q.phone)
        .ok_or_else(|| invalid("номер — не меньше 9 цифр"))?;
    let mut conn = state.pool.acquire().await?;
    // Клиенты общие для филиалов (SPEC-33), но ровно одна карточка: по общему номеру не
    // угадываем (ADR-063).
    let found = sqlx::query!(
        r#"select p.id, p.name, p.phone from parties p
           where p.role = 'customer' and p.active
             and phone_norm(p.phone) = $1
             and exists (select 1 from telegram_customers t where t.party_id = p.id)
           order by p.created_at desc limit 2"#,
        key
    )
    .fetch_all(&mut *conn)
    .await?;
    let r = match found.as_slice() {
        [one] => one,
        [] => {
            return Err(invalid(
                "клиент с этим номером не подключён к боту: пусть напишет боту и поделится номером",
            ));
        }
        _ => {
            return Err(invalid(
                "по этому номеру несколько карточек клиентов — оставьте номер в одной",
            ));
        }
    };
    let balance_tyiyn = balance(&mut conn, r.id).await?;
    Ok(Json(LookupOut {
        party_id: r.id,
        name: r.name.clone(),
        phone: r.phone.clone(),
        balance_tyiyn,
    }))
}

// ---------- История баллов клиента ----------

#[derive(Serialize)]
pub struct HistoryRow {
    pub kind: String,
    pub amount_tyiyn: i64,
    pub sale_number: Option<i64>,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct HistoryOut {
    pub member: bool,
    pub balance_tyiyn: i64,
    pub rows: Vec<HistoryRow>,
}

pub async fn history(conn: &mut PgConnection, party_id: Uuid, limit: i64) -> AppResult<HistoryOut> {
    let rows = sqlx::query_as!(
        HistoryRow,
        r#"select l.kind, l.amount_tyiyn, s.number as "sale_number?", l.created_at
           from loyalty_ledger l left join sales s on s.id = l.doc_id
           where l.party_id = $1 order by l.created_at desc limit $2"#,
        party_id,
        limit
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(HistoryOut {
        member: is_member(conn, party_id).await?,
        balance_tyiyn: balance(conn, party_id).await?,
        rows,
    })
}

async fn party_history(
    State(state): State<AppState>,
    _user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<HistoryOut>> {
    let mut conn = state.pool.acquire().await?;
    let found = sqlx::query_scalar!(
        r#"select exists (select 1 from parties where id = $1) as "e!""#,
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    if !found {
        return Err(AppError::NotFound);
    }
    Ok(Json(history(&mut conn, id, 50).await?))
}

// ---------- Настройка ----------

/// Изменение настройки: без `oil_change_only` — прежнее значение.
#[derive(Deserialize)]
struct SettingsReq {
    /// Повтор потерянного ответа — тот же ответ (I8).
    #[serde(default)]
    op_id: Option<Uuid>,
    rate_bp: i32,
    oil_change_only: Option<bool>,
}

async fn get_settings(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<Settings>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(settings(&mut conn, user.branch_id).await?))
}

async fn put_settings(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<SettingsReq>,
) -> AppResult<Json<Settings>> {
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if !(0..=1000).contains(&req.rate_bp) {
        return Err(invalid("процент начисления от 0 до 10"));
    }
    const KIND: &str = "settings.loyalty";
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_opt(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let before = settings(&mut tx, ctx.user.branch_id).await?;
    let out = Settings {
        rate_bp: req.rate_bp,
        oil_change_only: req.oil_change_only.unwrap_or(before.oil_change_only),
    };
    let value = json!({ "rate_bp": out.rate_bp, "oil_change_only": out.oil_change_only });
    sqlx::query!(
        r#"insert into settings (branch_id, key, value) values ($1, 'loyalty', $2)
           on conflict (branch_id, key) do update set value = excluded.value"#,
        ctx.user.branch_id,
        value
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(&mut tx, &ctx, KIND, "settings", None, value).await?;
    ops::finish_opt(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}
