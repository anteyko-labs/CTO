//! «Настройки → Масляная книжка и печать» (задача 003 Н5, ADR-{B3.1}): что касса печатает сама
//! после чека «В сервис», что идёт в чек книжки и в блок книжки товарного чека, как пишется масло
//! канистрой и разливом и идут ли фильтры на срок масла. У каждого филиала своя (`settings`, ключ
//! `oil_book`), правит только владелец, каждое изменение — в журнал (I5). По умолчанию — как было,
//! а фильтры в чеке книжки — да (задача 003 Н5).
//! Касса без сети берёт настройку из снимка (`offline.rs`).

use axum::extract::State;
use axum::{Json, Router, routing};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::auth::{Ctx, CurrentUser};
use crate::error::{AppError, AppResult, invalid};
use crate::ops;
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/settings/oil-book",
        routing::get(get_settings).put(put_settings),
    )
}

/// Масло канистрой и разливом одного товара — одной строкой: итог («4,5 л») или итог с разбивкой
/// («4,5 л (1 кан. × 4 л + 0,5 л)»).
pub const QTY_STYLES: [&str; 2] = ["total", "split"];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct OilBookSettings {
    /// После чека «В сервис» касса сама печатает чек книжки (как было).
    pub auto_book: bool,
    /// После чека «В сервис» касса сама печатает и товарный чек (раньше — только кнопкой).
    pub auto_receipt: bool,
    /// Блок «Масляная книжка» в товарном чеке (ADR-116).
    pub receipt_block: bool,
    /// В чеке книжки и в блоке — ещё фильтры (масляный, воздушный, салонный, топливный); по
    /// умолчанию — да: чек книжки печатает, что поменяли по виду замены (задача 003 Н5); выключено —
    /// только масла и жидкости (ADR-116).
    pub with_filters: bool,
    /// Запись чека — только фильтры, без жидкостей: печатать чек книжки (по умолчанию — нет,
    /// печатать нечего).
    pub print_filters_only: bool,
    /// Канистра и разлив одного товара: `total` — итог, `split` — итог с разбивкой.
    pub qty_style: String,
    /// Масляный, воздушный и салонный фильтр с заменой масла — на пробег масла (ADR-117).
    pub filters_follow_oil: bool,
}

impl Default for OilBookSettings {
    fn default() -> Self {
        Self {
            auto_book: true,
            auto_receipt: false,
            receipt_block: true,
            with_filters: true,
            print_filters_only: false,
            qty_style: "total".into(),
            filters_follow_oil: true,
        }
    }
}

/// Настройка филиала; нет записи или она непонятна — по умолчанию.
pub async fn load(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<OilBookSettings> {
    let v = sqlx::query_scalar!(
        "select value from settings where branch_id = $1 and key = 'oil_book'",
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    let mut s = v
        .and_then(|v| serde_json::from_value::<OilBookSettings>(v).ok())
        .unwrap_or_default();
    if !QTY_STYLES.contains(&s.qty_style.as_str()) {
        s.qty_style = OilBookSettings::default().qty_style;
    }
    Ok(s)
}

async fn get_settings(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<OilBookSettings>> {
    // Читают все (касса печатает по ней), правит только владелец.
    let mut conn = state.pool.acquire().await?;
    Ok(Json(load(&mut conn, user.branch_id).await?))
}

#[derive(Deserialize)]
struct SettingsIn {
    op_id: Uuid,
    #[serde(flatten)]
    settings: OilBookSettings,
}

async fn put_settings(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<SettingsIn>,
) -> AppResult<Json<OilBookSettings>> {
    ctx.user.require_owner()?;
    const KIND: &str = "settings.oil_book";
    let s = req.settings;
    if !QTY_STYLES.contains(&s.qty_style.as_str()) {
        return Err(invalid(
            "масло канистрой и разливом: итог или итог с разбивкой",
        ));
    }
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op::<OilBookSettings>(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let before = load(&mut tx, ctx.user.branch_id).await?;
    let value = serde_json::to_value(&s).map_err(|e| AppError::Internal(e.to_string()))?;
    sqlx::query!(
        r#"insert into settings (branch_id, key, value) values ($1, 'oil_book', $2)
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
