//! Операционные расходы по статьям (SPEC-06).

use axum::extract::{Path, Query, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::cash::{self, CashEntry};
use crate::auth::{Ctx, CurrentUser};
use crate::error::{AppError, AppResult, invalid};
use crate::ops::{self, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/expense-articles",
            routing::get(list_articles).post(create_article),
        )
        .route("/expense-articles/{id}", routing::patch(update_article))
        .route("/expenses", routing::get(list).post(post_expense))
        .route("/expenses/{id}/reverse", routing::post(reverse))
}

#[derive(Serialize)]
struct ArticleOut {
    id: Uuid,
    name: String,
    owner_only: bool,
    active: bool,
}

/// Новый филиал получает начальные статьи при первом обращении (ADR-023).
pub async fn ensure_articles(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<()> {
    let have = sqlx::query_scalar!(
        r#"select count(*) as "n!" from expense_articles where branch_id = $1"#,
        branch_id
    )
    .fetch_one(&mut *conn)
    .await?;
    if have > 0 {
        return Ok(());
    }
    for (name, owner_only) in [
        ("На развитие", false),
        ("Хозтовары", false),
        ("Налоги", false),
        ("Личные расходы", true),
        ("Прочее", false),
    ] {
        sqlx::query!(
            "insert into expense_articles (id, branch_id, name, owner_only) values ($1, $2, $3, $4) on conflict (branch_id, name) do nothing",
            new_id(),
            branch_id,
            name,
            owner_only
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

async fn list_articles(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<Vec<ArticleOut>>> {
    let mut conn = state.pool.acquire().await?;
    ensure_articles(&mut conn, user.branch_id).await?;
    // Статьи владельца администратору не видны (инвариант 13).
    let rows = sqlx::query_as!(
        ArticleOut,
        r#"select id, name, owner_only, active from expense_articles
           where branch_id = $1 and (not owner_only or $2)
           order by active desc, name"#,
        user.branch_id,
        user.is_owner()
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct ArticleReq {
    name: String,
    #[serde(default)]
    owner_only: bool,
}

async fn create_article(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<ArticleReq>,
) -> AppResult<Json<ArticleOut>> {
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(invalid("укажите название статьи"));
    }
    if name.chars().count() > 100 {
        return Err(invalid("название не длиннее 100 знаков"));
    }
    let id = new_id();
    // Статья и запись в журнал — одна транзакция (инвариант 7).
    let mut tx = state.pool.begin().await?;
    // Начальные статьи заводятся, только пока статей нет: первая своя их не отменяет.
    ensure_articles(&mut tx, ctx.user.branch_id).await?;
    sqlx::query!(
        "insert into expense_articles (id, branch_id, name, owner_only) values ($1, $2, $3, $4)",
        id,
        ctx.user.branch_id,
        name,
        req.owner_only
    )
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        if crate::error::is_unique_violation(&e) {
            AppError::Conflict("статья с таким названием уже есть".into())
        } else {
            AppError::from(e)
        }
    })?;
    ops::audit(
        &mut tx,
        &ctx,
        "expense.article_create",
        "expense_article",
        Some(id),
        json!({ "name": name, "owner_only": req.owner_only }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(ArticleOut {
        id,
        name,
        owner_only: req.owner_only,
        active: true,
    }))
}

#[derive(Deserialize)]
struct ArticlePatch {
    name: Option<String>,
    owner_only: Option<bool>,
    active: Option<bool>,
}

async fn update_article(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<ArticlePatch>,
) -> AppResult<Json<ArticleOut>> {
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    // Правка и запись в журнал — одна транзакция (инвариант 7).
    let mut tx = state.pool.begin().await?;
    let before = sqlx::query!(
        "select name, owner_only, active from expense_articles where id = $1 and branch_id = $2 for update",
        id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    let out = sqlx::query_as!(
        ArticleOut,
        r#"update expense_articles
           set name = coalesce(nullif(trim($3), ''), name),
               owner_only = coalesce($4, owner_only),
               active = coalesce($5, active)
           where id = $1 and branch_id = $2
           returning id, name, owner_only, active"#,
        id,
        ctx.user.branch_id,
        req.name,
        req.owner_only,
        req.active
    )
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        if crate::error::is_unique_violation(&e) {
            AppError::Conflict("статья с таким названием уже есть".into())
        } else {
            AppError::from(e)
        }
    })?;
    let changes = ops::changes(
        &json!({ "name": before.name, "owner_only": before.owner_only, "active": before.active }),
        &json!({ "name": out.name, "owner_only": out.owner_only, "active": out.active }),
    );
    ops::audit(
        &mut tx,
        &ctx,
        "expense.article_update",
        "expense_article",
        Some(id),
        json!({ "name": out.name, "changes": changes }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ExpenseOut {
    pub id: Uuid,
    pub number: i64,
    pub article_id: Uuid,
    pub article_name: String,
    pub amount_tyiyn: i64,
    pub source: String,
    pub account_name: Option<String>,
    pub expense_date: NaiveDate,
    pub comment: String,
    pub reversal_of: Option<Uuid>,
    pub reversed: bool,
    pub user_name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Deserialize, Default)]
pub(crate) struct ListQuery {
    pub(crate) from: Option<NaiveDate>,
    pub(crate) to: Option<NaiveDate>,
    pub(crate) article_id: Option<Uuid>,
}

async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ListQuery>,
    Query(page): Query<crate::api::paging::PageQuery>,
) -> AppResult<crate::api::paging::Paged<ExpenseOut>> {
    let mut conn = state.pool.acquire().await?;
    let (items, total) = page.slice(list_data(&mut conn, &user, q).await?, 500);
    Ok(crate::api::paging::paged(items, total))
}

/// Список расходов экрана «Расходы» — и для отчёта бота владельцу (SPEC-24).
pub(crate) async fn list_data(
    conn: &mut PgConnection,
    user: &CurrentUser,
    q: ListQuery,
) -> AppResult<Vec<ExpenseOut>> {
    // Администратору — только то, что он мог провести сам: из кассы, за последнюю неделю.
    let owner = user.is_owner();
    let rows = sqlx::query_as!(
        ExpenseOut,
        r#"select e.id, e.number, e.article_id, a.name as article_name, e.amount_tyiyn, e.source,
                  c.name as "account_name?", e.expense_date, e.comment, e.reversal_of,
                  exists (select 1 from expenses x where x.reversal_of = e.id) as "reversed!",
                  u.full_name as user_name, e.created_at
           from expenses e
           join expense_articles a on a.id = e.article_id
           join users u on u.id = e.user_id
           left join cash_accounts c on c.id = e.account_id
           where e.branch_id = $1
             and ($2::date is null or e.expense_date >= $2)
             and ($3::date is null or e.expense_date <= $3)
             and ($4::uuid is null or e.article_id = $4)
             and ($5 or (not a.owner_only and e.source = 'account'
                         and c.kind = 'register' and not c.owner_only
                         and e.expense_date >= (now() at time zone 'Asia/Bishkek')::date - 7))
           order by e.expense_date desc, e.created_at desc
           limit $6"#,
        user.branch_id,
        q.from,
        q.to,
        q.article_id,
        owner,
        crate::api::paging::MAX_ROWS
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows)
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct ExpenseReq {
    pub op_id: Uuid,
    pub article_id: Uuid,
    pub amount_tyiyn: i64,
    /// `account` — из кассы или со счёта. `outside` (не из денег точки) больше не принимается —
    /// 422 (ADR-079); прежние расходы «не из денег точки» остаются в истории.
    pub source: Option<String>,
    pub account_id: Option<Uuid>,
    pub expense_date: Option<NaiveDate>,
    #[serde(default)]
    pub comment: String,
}

async fn load_expense(conn: &mut PgConnection, branch_id: Uuid, id: Uuid) -> AppResult<ExpenseOut> {
    sqlx::query_as!(
        ExpenseOut,
        r#"select e.id, e.number, e.article_id, a.name as article_name, e.amount_tyiyn, e.source,
                  c.name as "account_name?", e.expense_date, e.comment, e.reversal_of,
                  exists (select 1 from expenses x where x.reversal_of = e.id) as "reversed!",
                  u.full_name as user_name, e.created_at
           from expenses e
           join expense_articles a on a.id = e.article_id
           join users u on u.id = e.user_id
           left join cash_accounts c on c.id = e.account_id
           where e.id = $1 and e.branch_id = $2"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)
}

async fn post_expense(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<ExpenseReq>,
) -> AppResult<Json<ExpenseOut>> {
    let mut tx = state.pool.begin().await?;
    let out = post_expense_tx(&mut tx, &ctx, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Расход по статье — одна транзакция вызывающего: экран и бот владельца (SPEC-24) проводят
/// одной функцией, с одними проверками.
pub async fn post_expense_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    req: ExpenseReq,
) -> AppResult<ExpenseOut> {
    const KIND: &str = "expense.post";
    if req.amount_tyiyn <= 0 {
        return Err(invalid("сумма больше нуля"));
    }
    crate::ops::check_amount(req.amount_tyiyn)?;
    let source = req.source.unwrap_or_else(|| "account".into());
    if source == "outside" {
        return Err(invalid(crate::api::receipts::NO_OUTSIDE));
    }
    if source != "account" {
        return Err(invalid("откуда платим: из кассы или со счёта"));
    }
    let owner = ctx.user.is_owner();
    let branch_id = ctx.user.branch_id;
    if let Some(done) = ops::begin_op(&mut *conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    ensure_articles(&mut *conn, branch_id).await?;
    let article = sqlx::query!(
        "select name, owner_only, active from expense_articles where id = $1 and branch_id = $2",
        req.article_id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| invalid("статья не найдена"))?;
    if !article.active {
        return Err(invalid("статья отключена"));
    }
    if article.owner_only && !owner {
        return Err(AppError::Forbidden);
    }
    if article.name == "Прочее" && req.comment.trim().is_empty() {
        return Err(invalid("по статье «Прочее» нужен комментарий"));
    }
    let today = sqlx::query_scalar!(r#"select (now() at time zone 'Asia/Bishkek')::date as "d!""#)
        .fetch_one(&mut *conn)
        .await?;
    let date = req.expense_date.unwrap_or(today);
    if date != today && !owner {
        return Err(AppError::Forbidden);
    }

    let mut account_id = None;
    if source == "account" {
        let id = match req.account_id {
            Some(id) => id,
            None => cash::default_account(&mut *conn, branch_id).await?,
        };
        // Администратор платит только из кассы смены: счета и сейфы — владельцу; архивный
        // счёт не выбирается (ADR-037, ADR-076).
        cash::check_pay_account(&mut *conn, &ctx.user, id).await?;
        account_id = Some(id);
    }

    let number = ops::next_counter(&mut *conn, branch_id, "expense").await?;
    let id = new_id();
    sqlx::query!(
        r#"insert into expenses (id, branch_id, number, article_id, amount_tyiyn, source,
                                 account_id, expense_date, comment, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)"#,
        id,
        branch_id,
        number,
        req.article_id,
        req.amount_tyiyn,
        source,
        account_id,
        date,
        req.comment.trim(),
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await?;
    if let Some(acc) = account_id {
        cash::require_open_shift(&mut *conn, branch_id, acc).await?;
        cash::add_movement(
            &mut *conn,
            ctx,
            CashEntry {
                account_id: acc,
                kind: "expense",
                amount: -req.amount_tyiyn,
                doc_type: "expense",
                doc_id: Some(id),
                comment: req.comment.trim(),
            },
        )
        .await?;
    }
    ops::audit(
        &mut *conn,
        ctx,
        KIND,
        "expense",
        Some(id),
        json!({ "article": article.name, "amount_tyiyn": req.amount_tyiyn, "from": source }),
    )
    .await?;
    let out = load_expense(&mut *conn, branch_id, id).await?;
    ops::finish_op(&mut *conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

#[derive(Deserialize)]
struct ReverseReq {
    op_id: Uuid,
    #[serde(default)]
    comment: String,
}

async fn reverse(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<ReverseReq>,
) -> AppResult<Json<ExpenseOut>> {
    const KIND: &str = "expense.reverse";
    let branch_id = ctx.user.branch_id;
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let orig = sqlx::query!(
        r#"select e.article_id, e.amount_tyiyn, e.source, e.account_id, e.expense_date, e.user_id,
                  e.reversal_of, a.owner_only,
                  exists (select 1 from expenses x where x.reversal_of = e.id) as "reversed!"
           from expenses e join expense_articles a on a.id = e.article_id
           where e.id = $1 and e.branch_id = $2
           for update of e"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    if orig.reversed || orig.reversal_of.is_some() {
        return Err(AppError::Conflict("расход уже сторнирован".into()));
    }
    // Администратор сторнирует только свой расход из кассы (SPEC-06).
    if !ctx.user.is_owner()
        && (orig.user_id != ctx.user.id || orig.source != "account" || orig.owner_only)
    {
        return Err(AppError::Forbidden);
    }
    // ...и только пока смена, в которой его провели, открыта. Расход вне смены — пока
    // смену так и не открыли: точка может работать и без смен (ADR-021).
    if !ctx.user.is_owner() {
        let acc = orig.account_id.ok_or(AppError::Forbidden)?;
        let shift = sqlx::query_scalar!(
            "select shift_id from cash_movements where doc_type = 'expense' and doc_id = $1 and account_id = $2",
            id,
            acc
        )
        .fetch_optional(&mut *tx)
        .await?
        .flatten();
        if shift != cash::open_shift_id(&mut tx, branch_id, acc).await? {
            return Err(AppError::Forbidden);
        }
    }
    let number = ops::next_counter(&mut tx, branch_id, "expense").await?;
    let rid = new_id();
    sqlx::query!(
        r#"insert into expenses (id, branch_id, number, article_id, amount_tyiyn, source,
                                 account_id, expense_date, comment, reversal_of, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)"#,
        rid,
        branch_id,
        number,
        orig.article_id,
        -orig.amount_tyiyn,
        orig.source,
        orig.account_id,
        orig.expense_date,
        req.comment.trim(),
        id,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        if crate::error::is_unique_violation(&e) {
            AppError::Conflict("расход уже сторнирован".into())
        } else {
            AppError::from(e)
        }
    })?;
    if let Some(acc) = orig.account_id {
        cash::require_open_shift(&mut tx, branch_id, acc).await?;
        cash::add_movement(
            &mut tx,
            &ctx,
            CashEntry {
                account_id: acc,
                kind: "reversal",
                amount: orig.amount_tyiyn,
                doc_type: "expense",
                doc_id: Some(rid),
                comment: req.comment.trim(),
            },
        )
        .await?;
    }
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "expense",
        Some(rid),
        json!({ "original": id, "amount_tyiyn": orig.amount_tyiyn }),
    )
    .await?;
    let out = load_expense(&mut tx, branch_id, rid).await?;
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}
