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
            "insert into expense_articles (id, branch_id, name, owner_only) values ($1, $2, $3, $4)",
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
    .fetch_all(&state.pool)
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
    let id = new_id();
    sqlx::query!(
        "insert into expense_articles (id, branch_id, name, owner_only) values ($1, $2, $3, $4)",
        id,
        ctx.user.branch_id,
        name,
        req.owner_only
    )
    .execute(&state.pool)
    .await?;
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
    sqlx::query_as!(
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
    .fetch_optional(&state.pool)
    .await?
    .map(Json)
    .ok_or(AppError::NotFound)
}

#[derive(Serialize, Deserialize, Clone)]
struct ExpenseOut {
    id: Uuid,
    number: i64,
    article_id: Uuid,
    article_name: String,
    amount_tyiyn: i64,
    source: String,
    account_name: Option<String>,
    expense_date: NaiveDate,
    comment: String,
    reversal_of: Option<Uuid>,
    reversed: bool,
    user_name: String,
    created_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct ListQuery {
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
    article_id: Option<Uuid>,
}

async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Vec<ExpenseOut>>> {
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
                         and e.expense_date >= (now() at time zone 'Asia/Bishkek')::date - 7))
           order by e.expense_date desc, e.created_at desc
           limit 500"#,
        user.branch_id,
        q.from,
        q.to,
        q.article_id,
        owner
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

#[derive(Deserialize)]
struct ExpenseReq {
    op_id: Uuid,
    article_id: Uuid,
    amount_tyiyn: i64,
    /// `account` — из кассы или со счёта, `outside` — не из денег точки.
    source: Option<String>,
    account_id: Option<Uuid>,
    expense_date: Option<NaiveDate>,
    #[serde(default)]
    comment: String,
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
    const KIND: &str = "expense.post";
    if req.amount_tyiyn <= 0 {
        return Err(invalid("сумма больше нуля"));
    }
    let source = req.source.unwrap_or_else(|| "account".into());
    if !matches!(source.as_str(), "account" | "outside") {
        return Err(invalid("откуда платим: из кассы или не из денег точки"));
    }
    let owner = ctx.user.is_owner();
    if source == "outside" && !owner {
        return Err(AppError::Forbidden);
    }
    let branch_id = ctx.user.branch_id;
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    ensure_articles(&mut tx, branch_id).await?;
    let article = sqlx::query!(
        "select name, owner_only, active from expense_articles where id = $1 and branch_id = $2",
        req.article_id,
        branch_id
    )
    .fetch_optional(&mut *tx)
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
        .fetch_one(&mut *tx)
        .await?;
    let date = req.expense_date.unwrap_or(today);
    if date != today && !owner {
        return Err(AppError::Forbidden);
    }

    let mut account_id = None;
    if source == "account" {
        let id = match req.account_id {
            Some(id) => id,
            None => cash::default_account(&mut tx, branch_id).await?,
        };
        let acc = sqlx::query!(
            "select kind, owner_only from cash_accounts where id = $1 and branch_id = $2",
            id,
            branch_id
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| invalid("касса не найдена"))?;
        // Администратор тратит только из кассы смены: счёт и сейфы — владельцу.
        if !owner && (acc.kind != "register" || acc.owner_only) {
            return Err(AppError::Forbidden);
        }
        account_id = Some(id);
    }

    let number = ops::next_counter(&mut tx, branch_id, "expense").await?;
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
    .execute(&mut *tx)
    .await?;
    if let Some(acc) = account_id {
        cash::add_movement(
            &mut tx,
            &ctx,
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
        &mut tx,
        &ctx,
        KIND,
        "expense",
        Some(id),
        json!({ "article": article.name, "amount_tyiyn": req.amount_tyiyn, "source": source }),
    )
    .await?;
    let out = load_expense(&mut tx, branch_id, id).await?;
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
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
           where e.id = $1 and e.branch_id = $2"#,
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
    .await?;
    if let Some(acc) = orig.account_id {
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
