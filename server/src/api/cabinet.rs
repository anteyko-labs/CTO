//! Кабинет юрлица: вход по ИНН, смена пароля при первом входе, просмотр своих покупок,
//! работников, долга и акта сверки (SPEC-12, ADR-049). Только чтение.

use axum::extract::{FromRequestParts, Path, Query, State};
use axum::http::header::SET_COOKIE;
use axum::http::request::Parts;
use axum::response::{AppendHeaders, IntoResponse};
use axum::{Json, Router, routing};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::api::auth::reserve_attempt;
use crate::api::parties::{load_party, reconciliation_data};
use crate::auth::{self, Ctx, Owner, hash_password, new_token, token_hash, verify_password};
use crate::error::{AppError, AppResult, invalid};
use crate::ops;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/client/login", routing::post(login))
        .route("/client/logout", routing::post(logout))
        .route("/client/password", routing::post(change_password))
        .route("/client/me", routing::get(me))
        .route("/client/sales", routing::get(sales))
        .route("/client/contacts", routing::get(contacts))
        .route("/client/reconciliation", routing::get(reconciliation))
        .route("/client/oil-book", routing::get(oil_book))
        .route("/parties/{id}/cabinet", routing::get(cabinet_state))
        .route("/parties/{id}/cabinet/reset", routing::post(cabinet_reset))
}

/// Начальный пароль кабинета (решение заказчика). При первом входе его обязательно меняют.
pub const DEFAULT_PASSWORD: &str = "avtodom2026";
const CLIENT_COOKIE: &str = "avtodom_client";
const CLIENT_DAYS: i64 = 30;

fn client_cookie(state: &AppState, value: &str, max_age: i64) -> String {
    let secure = if state.cookie_secure { "; Secure" } else { "" };
    format!("{CLIENT_COOKIE}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age}{secure}")
}

/// Вошедшее юрлицо. Пока пароль не сменён, доступны только смена пароля и выход.
pub struct Client {
    pub party_id: Uuid,
    pub branch_id: Uuid,
    pub must_change: bool,
}

impl FromRequestParts<AppState> for Client {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = auth::cookie_value(parts, CLIENT_COOKIE).ok_or(AppError::Unauthorized)?;
        let r = sqlx::query!(
            r#"select s.party_id, p.branch_id, a.must_change
               from client_sessions s
               join parties p on p.id = s.party_id
               join party_accounts a on a.party_id = s.party_id
               where s.token_hash = $1 and s.expires_at > now() and p.active"#,
            token_hash(token)
        )
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::Unauthorized)?;
        Ok(Client {
            party_id: r.party_id,
            branch_id: r.branch_id,
            must_change: r.must_change,
        })
    }
}

impl Client {
    /// Данные — только после смены начального пароля.
    fn ready(&self) -> AppResult<()> {
        if self.must_change {
            return Err(AppError::Validation("сначала смените пароль".into()));
        }
        Ok(())
    }
}

/// Запись в журнал от имени кабинета: пользователя точки здесь нет.
async fn audit(
    conn: &mut sqlx::PgConnection,
    branch_id: Uuid,
    party_id: Uuid,
    action: &str,
) -> AppResult<()> {
    sqlx::query!(
        r#"insert into audit_log (id, branch_id, action, entity, entity_id, data)
           values ($1, $2, $3, 'party', $4, '{}')"#,
        ops::new_id(),
        branch_id,
        action,
        party_id
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

#[derive(Deserialize)]
struct LoginReq {
    inn: String,
    password: String,
}

#[derive(Serialize)]
struct LoginOut {
    name: String,
    must_change: bool,
}

async fn login(
    State(state): State<AppState>,
    parts: Parts,
    Json(req): Json<LoginReq>,
) -> AppResult<impl IntoResponse> {
    let inn = req.inn.trim().to_string();
    if inn.is_empty() || inn.len() > 20 || req.password.len() > 200 || inn.contains('\0') {
        return Err(AppError::Unauthorized);
    }
    // Подбор пароля ограничен так же, как у сотрудников (ADR-016).
    let key = format!("client:{inn}|{}", auth::client_addr(&parts));
    if let Some(minutes) = reserve_attempt(&state, &key).await? {
        return Err(AppError::TooManyRequests(format!(
            "слишком много неудачных попыток, повторите через {minutes} мин"
        )));
    }
    let party = sqlx::query!(
        r#"select p.id, p.branch_id, p.name, a.password_hash as "hash?", a.must_change as "must_change?"
           from parties p left join party_accounts a on a.party_id = p.id
           where p.inn = $1 and p.kind = 'company' and p.role = 'customer' and p.active
           order by p.created_at limit 1"#,
        inn
    )
    .fetch_optional(&state.pool)
    .await?;
    // Проверка идёт и для неизвестного ИНН, чтобы время ответа его не выдавало.
    let ok = match party.as_ref() {
        Some(p) => match &p.hash {
            Some(h) => verify_password(req.password.clone(), h.clone()).await,
            None => req.password == DEFAULT_PASSWORD,
        },
        None => {
            verify_password(req.password.clone(), state.dummy_hash.as_ref().clone()).await;
            false
        }
    };
    let p = match party {
        Some(p) if ok => p,
        _ => return Err(AppError::Unauthorized),
    };
    let mut tx = state.pool.begin().await?;
    if p.hash.is_none() {
        // Первый вход с начальным паролем: кабинет заводится, пароль сменят сразу.
        let hash = hash_password(DEFAULT_PASSWORD.into()).await?;
        sqlx::query!(
            r#"insert into party_accounts (party_id, password_hash, must_change) values ($1, $2, true)
               on conflict (party_id) do nothing"#,
            p.id,
            hash
        )
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query!("delete from login_attempts where login = $1", key)
        .execute(&mut *tx)
        .await?;
    sqlx::query!(
        "update party_accounts set last_login_at = now() where party_id = $1",
        p.id
    )
    .execute(&mut *tx)
    .await?;
    let (token, hash) = new_token();
    sqlx::query!(
        "insert into client_sessions (token_hash, party_id, expires_at) values ($1, $2, now() + make_interval(days => $3))",
        hash,
        p.id,
        i32::try_from(CLIENT_DAYS).unwrap_or(30)
    )
    .execute(&mut *tx)
    .await?;
    audit(&mut tx, p.branch_id, p.id, "cabinet.login").await?;
    tx.commit().await?;
    let cookie = client_cookie(&state, &token, CLIENT_DAYS * 86_400);
    Ok((
        AppendHeaders([(SET_COOKIE, cookie)]),
        Json(LoginOut {
            name: p.name,
            must_change: p.must_change.unwrap_or(true),
        }),
    ))
}

async fn logout(State(state): State<AppState>, parts: Parts) -> AppResult<impl IntoResponse> {
    if let Some(token) = auth::cookie_value(&parts, CLIENT_COOKIE) {
        sqlx::query!(
            "delete from client_sessions where token_hash = $1",
            token_hash(token)
        )
        .execute(&state.pool)
        .await?;
    }
    Ok((
        AppendHeaders([(SET_COOKIE, client_cookie(&state, "", 0))]),
        Json(json!({})),
    ))
}

#[derive(Deserialize)]
struct PasswordReq {
    old_password: String,
    new_password: String,
}

async fn change_password(
    State(state): State<AppState>,
    client: Client,
    parts: Parts,
    Json(req): Json<PasswordReq>,
) -> AppResult<Json<Value>> {
    let new = req.new_password.trim().to_string();
    if new.chars().count() < 8 || new.len() > 200 {
        return Err(invalid("новый пароль — не короче 8 знаков"));
    }
    if new == DEFAULT_PASSWORD {
        return Err(invalid("придумайте свой пароль, не начальный"));
    }
    let hash = sqlx::query_scalar!(
        "select password_hash from party_accounts where party_id = $1",
        client.party_id
    )
    .fetch_one(&state.pool)
    .await?;
    if !verify_password(req.old_password, hash).await {
        return Err(invalid("текущий пароль неверный"));
    }
    let new_hash = hash_password(new).await?;
    let mut tx = state.pool.begin().await?;
    sqlx::query!(
        "update party_accounts set password_hash = $2, must_change = false, updated_at = now() where party_id = $1",
        client.party_id,
        new_hash
    )
    .execute(&mut *tx)
    .await?;
    // Остальные устройства выходят: пароль сменили — старые входы больше не действуют.
    let current = auth::cookie_value(&parts, CLIENT_COOKIE).map(token_hash);
    sqlx::query!(
        "delete from client_sessions where party_id = $1 and token_hash is distinct from $2",
        client.party_id,
        current
    )
    .execute(&mut *tx)
    .await?;
    audit(
        &mut tx,
        client.branch_id,
        client.party_id,
        "cabinet.password",
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Serialize)]
struct MeOut {
    name: String,
    inn: String,
    phone: String,
    must_change: bool,
    balance_tyiyn: i64,
    overdue_tyiyn: i64,
    due_days: Option<i32>,
    credit_limit_tyiyn: Option<i64>,
    /// Реквизиты точки — для шапки акта сверки при печати.
    seller: Value,
}

async fn me(State(state): State<AppState>, client: Client) -> AppResult<Json<MeOut>> {
    let mut conn = state.pool.acquire().await?;
    let p = load_party(&mut conn, client.branch_id, client.party_id).await?;
    let seller = sqlx::query_scalar!(
        "select value from settings where branch_id = $1 and key = 'debt_docs'",
        client.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .and_then(|v| v.get("seller").cloned())
    .unwrap_or_else(|| json!({}));
    Ok(Json(MeOut {
        name: p.name,
        inn: p.inn,
        phone: p.phone,
        must_change: client.must_change,
        balance_tyiyn: p.balance_tyiyn,
        overdue_tyiyn: p.overdue_tyiyn,
        due_days: p.due_days,
        credit_limit_tyiyn: p.credit_limit_tyiyn,
        seller,
    }))
}

#[derive(Deserialize)]
struct PeriodQuery {
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
}

#[derive(Serialize)]
struct LineOut {
    name: String,
    kind: String,
    qty: i64,
    unit: String,
    container_ml: Option<i64>,
    amount_tyiyn: i64,
}

#[derive(Serialize)]
struct SaleOut {
    id: Uuid,
    number: i64,
    kind: String,
    business_date: NaiveDate,
    created_at: DateTime<Utc>,
    contact_name: Option<String>,
    vehicle_plate: Option<String>,
    total_tyiyn: i64,
    debt_tyiyn: i64,
    lines: Vec<LineOut>,
}

/// Покупки фирмы: кто из работников приезжал, когда, на какой машине и что взял.
/// Только цены продажи — себестоимость клиенту не показывается никогда.
async fn sales(
    State(state): State<AppState>,
    client: Client,
    Query(q): Query<PeriodQuery>,
) -> AppResult<Json<Vec<SaleOut>>> {
    client.ready()?;
    let mut conn = state.pool.acquire().await?;
    let heads = sqlx::query!(
        r#"select s.id, s.number, s.kind, s.business_date, s.created_at, s.total_tyiyn,
                  c.full_name as "contact_name?", v.plate as "vehicle_plate?",
                  coalesce((select sum(p.amount_tyiyn) from sale_payments p
                            where p.sale_id = s.id and p.method = 'debt'), 0)::bigint as "debt!"
           from sales s
           left join party_contacts c on c.id = s.contact_id
           left join party_vehicles v on v.id = s.vehicle_id
           where s.party_id = $1 and s.branch_id = $2
             and s.business_date >= coalesce($3, (now() at time zone 'Asia/Bishkek')::date - 90)
             and s.business_date <= coalesce($4, (now() at time zone 'Asia/Bishkek')::date)
           order by s.created_at desc limit 500"#,
        client.party_id,
        client.branch_id,
        q.from,
        q.to
    )
    .fetch_all(&mut *conn)
    .await?;
    let ids: Vec<Uuid> = heads.iter().map(|h| h.id).collect();
    let lines = sqlx::query!(
        r#"select l.sale_id, coalesce(p.name, sv.name) as "name!", l.kind, l.qty,
                  coalesce(p.unit, 'piece') as "unit!", p.container_ml as "container_ml?", l.amount_tyiyn
           from sale_lines l
           left join products p on p.id = l.product_id
           left join services sv on sv.id = l.service_id
           where l.sale_id = any($1) order by l.line_no"#,
        &ids
    )
    .fetch_all(&mut *conn)
    .await?;
    let out = heads
        .into_iter()
        .map(|h| SaleOut {
            lines: lines
                .iter()
                .filter(|l| l.sale_id == h.id)
                .map(|l| LineOut {
                    name: l.name.clone(),
                    kind: l.kind.clone(),
                    qty: l.qty,
                    unit: l.unit.clone(),
                    container_ml: l.container_ml,
                    amount_tyiyn: l.amount_tyiyn,
                })
                .collect(),
            id: h.id,
            number: h.number,
            kind: h.kind,
            business_date: h.business_date,
            created_at: h.created_at,
            contact_name: h.contact_name,
            vehicle_plate: h.vehicle_plate,
            total_tyiyn: h.total_tyiyn,
            debt_tyiyn: h.debt,
        })
        .collect();
    Ok(Json(out))
}

#[derive(Serialize)]
struct ContactOut {
    full_name: String,
    phone: String,
    position: String,
    visits: i64,
    last_at: Option<DateTime<Utc>>,
    total_tyiyn: i64,
    debt_tyiyn: i64,
}

/// Работники фирмы: сколько раз приезжали, когда последний раз, на сколько взяли и сколько
/// из этого в долг — видно, кто из них набирает долг.
async fn contacts(
    State(state): State<AppState>,
    client: Client,
) -> AppResult<Json<Vec<ContactOut>>> {
    client.ready()?;
    let rows = sqlx::query_as!(
        ContactOut,
        r#"select c.full_name, c.phone, c.position,
                  count(s.id) filter (where s.kind = 'sale') as "visits!",
                  max(s.created_at) as "last_at?",
                  coalesce(sum(s.total_tyiyn), 0)::bigint as "total_tyiyn!",
                  coalesce(sum((select coalesce(sum(p.amount_tyiyn), 0) from sale_payments p
                                where p.sale_id = s.id and p.method = 'debt')), 0)::bigint as "debt_tyiyn!"
           from party_contacts c
           left join sales s on s.contact_id = c.id and s.branch_id = $2
           where c.party_id = $1
           group by c.id, c.full_name, c.phone, c.position
           order by max(s.created_at) desc nulls last, c.full_name"#,
        client.party_id,
        client.branch_id
    )
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(rows))
}

async fn reconciliation(
    State(state): State<AppState>,
    client: Client,
    Query(q): Query<PeriodQuery>,
) -> AppResult<Json<Value>> {
    client.ready()?;
    let mut conn = state.pool.acquire().await?;
    let act =
        reconciliation_data(&mut conn, client.branch_id, client.party_id, q.from, q.to).await?;
    Ok(Json(json!(act)))
}

/// Машины фирмы: когда меняли масло, что залили и когда следующая замена (SPEC-16).
async fn oil_book(State(state): State<AppState>, client: Client) -> AppResult<Json<Value>> {
    client.ready()?;
    let mut conn = state.pool.acquire().await?;
    let books =
        crate::api::oil_book::books(&mut conn, client.branch_id, Some(client.party_id), None)
            .await?;
    Ok(Json(json!(books)))
}

// ---------- Владелец: состояние кабинета и сброс пароля ----------

#[derive(Serialize)]
struct CabinetState {
    /// Кабинет есть у юрлица-клиента с ИНН: ИНН — это логин.
    available: bool,
    login: String,
    has_account: bool,
    must_change: bool,
    last_login_at: Option<DateTime<Utc>>,
}

async fn cabinet_state(
    State(state): State<AppState>,
    Owner(user): Owner,
    Path(id): Path<Uuid>,
) -> AppResult<Json<CabinetState>> {
    let r = sqlx::query!(
        r#"select p.kind, p.role, p.inn, a.must_change as "must_change?", a.last_login_at
           from parties p left join party_accounts a on a.party_id = p.id
           where p.id = $1 and p.branch_id = $2"#,
        id,
        user.branch_id
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    Ok(Json(CabinetState {
        available: r.kind == "company" && r.role == "customer" && !r.inn.trim().is_empty(),
        login: r.inn,
        has_account: r.must_change.is_some(),
        must_change: r.must_change.unwrap_or(true),
        last_login_at: r.last_login_at,
    }))
}

/// Сброс пароля кабинета на начальный — только владелец (решение заказчика). Все входы фирмы
/// закрываются, при следующем входе пароль снова нужно сменить.
async fn cabinet_reset(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let kind = sqlx::query!(
        "select kind, role, inn from parties where id = $1 and branch_id = $2",
        id,
        ctx.user.branch_id
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;
    if kind.kind != "company" || kind.role != "customer" || kind.inn.trim().is_empty() {
        return Err(invalid("кабинет есть только у юрлица-клиента с ИНН"));
    }
    let hash = hash_password(DEFAULT_PASSWORD.into()).await?;
    let mut tx = state.pool.begin().await?;
    sqlx::query!(
        r#"insert into party_accounts (party_id, password_hash, must_change) values ($1, $2, true)
           on conflict (party_id) do update set password_hash = excluded.password_hash,
             must_change = true, updated_at = now()"#,
        id,
        hash
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!("delete from client_sessions where party_id = $1", id)
        .execute(&mut *tx)
        .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "cabinet.reset",
        "party",
        Some(id),
        json!({ "login": kind.inn }),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "ok": true })))
}
