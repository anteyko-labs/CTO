//! Подбор кросс-номеров фильтра сервисом разбора (задача 002 П10, ADR-145; сервис — как
//! ADR-098). «Подобрать кросс-номера» в карточке товара и для всей категории (касса вызывает подбор
//! по товарам по очереди). Режим — настройка филиала: «записывать сразу» (по умолчанию, ответ
//! заказчика 30.10.2026) — номера сразу в карточке с пометкой «подобрано автоматически» и в
//! журнале; «предлагать» — номера возвращаются, сохраняет человек (`PUT /products/{id}/cross`).
//! Дневной лимит обращений филиала — в настройке. Каждое обращение — строка `cross_picks`; повтор
//! `op_id` возвращает тот же ответ (I8).

use axum::extract::{Path, State};
use axum::{Json, Router, routing};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::analogs::{load_cross, norm};
use crate::api::assistant;
use crate::auth::{Ctx, CurrentUser};
use crate::error::{AppError, AppResult, invalid};
use crate::ops;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/settings/cross-pick",
            routing::get(get_settings).put(put_settings),
        )
        .route("/products/{id}/cross/pick", routing::post(pick))
}

/// Режимы: записывать сразу или предлагать (сохраняет человек).
pub const MODES: [&str; 2] = ["auto", "suggest"];
/// Номеров у товара — не больше, как в карточке (`analogs::put_cross`).
const MAX_CODES: usize = 100;
/// Из одного ответа — не больше 30 новых номеров.
const MAX_FOUND: usize = 30;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct CrossPickSettings {
    pub mode: String,
    /// Обращений к сервису за день на филиал; 0 — подбор выключен.
    pub daily_limit: i64,
}

impl Default for CrossPickSettings {
    fn default() -> Self {
        Self {
            mode: "auto".into(),
            daily_limit: 50,
        }
    }
}

pub async fn load(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<CrossPickSettings> {
    let v = sqlx::query_scalar!(
        "select value from settings where branch_id = $1 and key = 'cross_pick'",
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    let mut s = v
        .and_then(|v| serde_json::from_value::<CrossPickSettings>(v).ok())
        .unwrap_or_default();
    if !MODES.contains(&s.mode.as_str()) {
        s.mode = CrossPickSettings::default().mode;
    }
    Ok(s)
}

/// Обращений к сервису за сегодня (день Бишкека) у филиала.
async fn used_today(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<i64> {
    Ok(sqlx::query_scalar!(
        r#"select count(*) as "n!" from cross_picks
           where branch_id = $1
             and created_at >= (date_trunc('day', now() at time zone 'Asia/Bishkek') at time zone 'Asia/Bishkek')"#,
        branch_id
    )
    .fetch_one(&mut *conn)
    .await?)
}

#[derive(Serialize)]
struct SettingsOut {
    #[serde(flatten)]
    settings: CrossPickSettings,
    used_today: i64,
    /// Сервис разбора настроен на сервере (`ASSISTANT_API_KEY`).
    available: bool,
}

async fn get_settings(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<SettingsOut>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(SettingsOut {
        settings: load(&mut conn, user.branch_id).await?,
        used_today: used_today(&mut conn, user.branch_id).await?,
        available: state.assistant.is_some(),
    }))
}

#[derive(Deserialize)]
struct SettingsIn {
    op_id: Uuid,
    #[serde(flatten)]
    settings: CrossPickSettings,
}

async fn put_settings(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<SettingsIn>,
) -> AppResult<Json<CrossPickSettings>> {
    ctx.user.require_owner()?;
    const KIND: &str = "settings.cross_pick";
    let s = req.settings;
    if !MODES.contains(&s.mode.as_str()) {
        return Err(invalid("режим подбора: записывать сразу или предлагать"));
    }
    if !(0..=1000).contains(&s.daily_limit) {
        return Err(invalid("дневной лимит подбора — от 0 до 1000"));
    }
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op::<CrossPickSettings>(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let before = load(&mut tx, ctx.user.branch_id).await?;
    let value = serde_json::to_value(&s).map_err(|e| AppError::Internal(e.to_string()))?;
    sqlx::query!(
        r#"insert into settings (branch_id, key, value) values ($1, 'cross_pick', $2)
           on conflict (branch_id, key) do update set value = excluded.value"#,
        ctx.user.branch_id,
        value
    )
    .execute(&mut *tx)
    .await?;
    if before != s {
        let before_v =
            serde_json::to_value(&before).map_err(|e| AppError::Internal(e.to_string()))?;
        ops::audit(
            &mut tx,
            &ctx,
            KIND,
            "settings",
            None,
            json!({ "changes": ops::changes(&before_v, &value) }),
        )
        .await?;
    }
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &s).await?;
    tx.commit().await?;
    Ok(Json(s))
}

#[derive(Deserialize)]
struct PickReq {
    op_id: Uuid,
}

#[derive(Serialize, Deserialize, Clone)]
struct PickOut {
    product_id: Uuid,
    mode: String,
    /// Новые номера из ответа сервиса (которых у товара ещё нет).
    found: Vec<String>,
    /// Записанные в карточку (режим «записывать сразу»).
    added: Vec<String>,
    /// Номера товара после подбора и подобранные сервисом среди них.
    codes: Vec<String>,
    auto: Vec<String>,
    used_today: i64,
    daily_limit: i64,
    /// Сервис не ответил или ответил не по схеме — для человека; номера не тронуты.
    error: Option<String>,
}

const SYSTEM: &str = "Ты подбираешь кросс-номера для автомобильных фильтров точки замены масла: оригинальные номера производителя машины (OEM) и номера аналогов других производителей фильтров (MANN, MAHLE, Knecht, Bosch, Filtron, Sakura, VIC, Hengst, UFI и других). Пиши только номера, в которых уверен; не придумывай и не угадывай. Не знаешь товар или не уверен — верни пустой список. Номер — как его пишет производитель, без названия товара.";

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "codes": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "brand": { "type": "string" },
                        "code": { "type": "string" }
                    },
                    "required": ["brand", "code"]
                }
            }
        },
        "required": ["codes"]
    })
}

/// Номера из ответа: непустые, до 40 знаков, с буквами или цифрами, без повторов, без своего
/// артикула и уже записанных у товара.
fn codes_of(answer: &Value, skip: &[String]) -> Vec<String> {
    let mut out: Vec<(String, String)> = Vec::new();
    for c in answer
        .get("codes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let raw = c
            .get("code")
            .and_then(Value::as_str)
            .or_else(|| c.as_str())
            .unwrap_or_default()
            .trim();
        let n = norm(raw);
        if raw.is_empty() || raw.chars().count() > 40 || n.is_empty() || skip.contains(&n) {
            continue;
        }
        if out.iter().any(|(x, _)| *x == n) {
            continue;
        }
        out.push((n, raw.to_string()));
        if out.len() >= MAX_FOUND {
            break;
        }
    }
    out.into_iter().map(|(_, c)| c).collect()
}

async fn pick(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<PickReq>,
) -> AppResult<Json<PickOut>> {
    let branch_id = ctx.user.branch_id;
    let mut tx = state.pool.begin().await?;
    // Кросс-номера — часть карточки товара: право `products` (ADR-097).
    crate::auth::require_permission(&mut tx, &ctx.user, "products").await?;
    sqlx::query!(
        "select pg_advisory_xact_lock(hashtextextended($1::text, 0))",
        format!("cross_pick:{branch_id}")
    )
    .execute(&mut *tx)
    .await?;
    if let Some(r) = sqlx::query!(
        "select user_id, product_id, status, response from cross_picks where op_id = $1",
        req.op_id
    )
    .fetch_optional(&mut *tx)
    .await?
    {
        if r.user_id != ctx.user.id || r.product_id != id {
            return Err(AppError::Conflict(
                "op_id уже использован другой операцией".into(),
            ));
        }
        return match r
            .response
            .and_then(|v| serde_json::from_value::<PickOut>(v).ok())
        {
            Some(out) => Ok(Json(out)),
            None => Err(AppError::Conflict(
                "подбор по этому товару ещё идёт — подождите".into(),
            )),
        };
    }
    let p = sqlx::query!(
        r#"select p.name, p.brand, p.article, p.attrs ->> 'filter_type' as filter_type,
                  c.kind, c.name as category
           from products p join categories c on c.id = p.category_id
           where p.id = $1"#,
        id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    if p.kind != "filter" {
        return Err(invalid("кросс-номера подбираются только у фильтров"));
    }
    let Some(a) = state.assistant.clone() else {
        return Err(invalid(
            "подбор кросс-номеров недоступен: на сервере не задан ключ сервиса разбора",
        ));
    };
    let settings = load(&mut tx, branch_id).await?;
    let used = used_today(&mut tx, branch_id).await?;
    if used >= settings.daily_limit {
        return Err(AppError::Conflict(format!(
            "дневной лимит подбора кросс-номеров исчерпан: {used} из {}. Поменять — «Настройки → Кросс-номера»",
            settings.daily_limit
        )));
    }
    let pick_id = ops::new_id();
    sqlx::query!(
        r#"insert into cross_picks (id, op_id, branch_id, product_id, user_id)
           values ($1, $2, $3, $4, $5)"#,
        pick_id,
        req.op_id,
        branch_id,
        id,
        ctx.user.id
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    // Обращение к сервису — вне транзакции: ответ идёт до минуты.
    let prompt = format!(
        "Фильтр: {}\nБренд: {}\nАртикул: {}\nКатегория: {}\nТип: {}\nВерни кросс-номера этого фильтра (до 20).",
        p.name,
        p.brand,
        p.article,
        p.category,
        p.filter_type.as_deref().unwrap_or("не указан")
    );
    let read = assistant::read_text(&a, SYSTEM, &prompt, &schema()).await;

    let mut tx = state.pool.begin().await?;
    sqlx::query_scalar!("select id from products where id = $1 for update", id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;
    let before = load_cross(&mut tx, id).await?;
    let mut skip: Vec<String> = before.codes.iter().map(|c| norm(c)).collect();
    let own = norm(&p.article);
    if !own.is_empty() {
        skip.push(own);
    }
    let (found, error) = match &read.result {
        Ok(v) => (codes_of(v, &skip), None),
        Err(e) => {
            tracing::warn!(error = %e, "подбор кросс-номеров: нет ответа");
            (
                Vec::new(),
                Some("сервис разбора не ответил — попробуйте позже".to_string()),
            )
        }
    };
    let mut added: Vec<String> = Vec::new();
    if settings.mode == "auto" {
        let room = MAX_CODES.saturating_sub(before.codes.len());
        for code in found.iter().take(room) {
            sqlx::query!(
                r#"insert into product_cross_numbers (product_id, code, code_norm, auto)
                   values ($1, $2, $3, true) on conflict do nothing"#,
                id,
                code,
                norm(code)
            )
            .execute(&mut *tx)
            .await?;
            added.push(code.clone());
        }
        if !added.is_empty() {
            let after = load_cross(&mut tx, id).await?;
            ops::audit(
                &mut tx,
                &ctx,
                "product.cross",
                "product",
                Some(id),
                json!({ "codes": after.codes, "auto": added, "note": "подобрано автоматически" }),
            )
            .await?;
        }
    }
    let now = load_cross(&mut tx, id).await?;
    let out = PickOut {
        product_id: id,
        mode: settings.mode.clone(),
        found: found.clone(),
        added: added.clone(),
        codes: now.codes,
        auto: now.auto,
        used_today: used.saturating_add(1),
        daily_limit: settings.daily_limit,
        error: error.clone(),
    };
    let response = serde_json::to_value(&out).map_err(|e| AppError::Internal(e.to_string()))?;
    sqlx::query!(
        r#"update cross_picks set status = $2, found = $3, added = $4, error = $5,
                  input_tokens = $6, output_tokens = $7, response = $8, finished_at = now()
           where id = $1"#,
        pick_id,
        if read.result.is_ok() {
            "done"
        } else {
            "failed"
        },
        json!(found),
        json!(added),
        read.result.as_ref().err().map(String::as_str),
        read.input_tokens,
        read.output_tokens,
        response
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_skip_own_article_known_and_repeats() {
        let answer = json!({ "codes": [
            { "brand": "MANN", "code": "W 712/75" },
            { "brand": "MANN", "code": "W712/75" },
            { "brand": "Toyota", "code": "90915-YZZE1" },
            { "brand": "Bosch", "code": "" },
            { "brand": "Свой", "code": "OC 217" },
            { "brand": "Длинный", "code": "1234567890123456789012345678901234567890X" }
        ]});
        let skip = vec![norm("OC217")];
        assert_eq!(codes_of(&answer, &skip), vec!["W 712/75", "90915-YZZE1"]);
        assert!(codes_of(&json!({}), &[]).is_empty());
    }
}
