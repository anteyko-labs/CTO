//! «Настройки → Касса» (задача 003 Н2–Н4, ADR-146, ADR-147, ADR-148): раскладка кнопок
//! режима кассы, высота плиток ходового и купюры у «Получено»; скидка на кассе (быстрые проценты,
//! предел скидки администратора); прибыль на бумаге и на экране чека в режиме кассы.
//! Хранение — `settings` филиала, ключ `cashier`; читают все вошедшие (касса без сети — из снимка),
//! меняет только владелец, каждое изменение — в журнал (I5), повтор `op_id` — тот же ответ (I8).
//! Права на кнопки не меняются: кнопка без права не показывается, где бы она ни стояла (I12).

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
        "/settings/cashier",
        routing::get(get_settings).put(put_settings),
    )
}

/// Места кнопки: под поиском, у «Итого», в колонке оплаты (под клиентом, как было), в окне
/// «Ещё действия…», скрыта; у кнопок полосы — в полосе сверху.
const ACTION: &[&str] = &["search", "total", "pay", "more", "hidden"];
const BAR: &[&str] = &["bar", "more", "hidden"];
/// «Выйти из режима кассы» не скрывается: без неё из режима не выйти.
const EXIT: &[&str] = &["bar", "more"];
/// «Наличные ↔ QR» (задача 003): и в полосе сверху, и среди действий.
const PAY_FIX: &[&str] = &["bar", "search", "total", "pay", "more", "hidden"];

/// Кнопки режима кассы: ключ, допустимые места, место по умолчанию — нынешняя раскладка
/// (SPEC-29 «Раскладка»). Порядок списка — порядок по умолчанию. Тот же список — в
/// `web/src/lib/cashierLayout.ts` (тест сверяет ключи и места по умолчанию).
pub const BUTTONS: [(&str, &[&str], &str); 16] = [
    ("debt_payment", ACTION, "search"),
    ("expense", ACTION, "search"),
    ("battery_intake", ACTION, "search"),
    ("battery_sell", ACTION, "search"),
    ("staff_pay", ACTION, "search"),
    ("no_receipt", ACTION, "search"),
    ("delivery", ACTION, "pay"),
    ("comment", ACTION, "pay"),
    ("battery_pay", ACTION, "pay"),
    ("bonus", ACTION, "pay"),
    // Новая кнопка (задача 003 Н3) — рядом с баллами, чтобы экран кассы остался прежним; набор
    // «Частое — у итога» ставит её к «Итого».
    ("discount", ACTION, "pay"),
    ("shift_print", BAR, "bar"),
    ("keyboard", BAR, "bar"),
    ("fullscreen", BAR, "bar"),
    ("exit", EXIT, "bar"),
    // «Наличные ↔ QR» открытой смены (задача 003 Н1): по умолчанию скрыта — экран прежний.
    ("pay_fix", PAY_FIX, "hidden"),
];

/// Место одной кнопки; порядок в списке — порядок на экране внутри места.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ButtonPlace {
    pub id: String,
    pub place: String,
}

/// Настройка кассы филиала. Нет записи или поля — значение по умолчанию (как было до задачи 003,
/// кроме прямых просьб заказчика: прибыль не печатается и не видна на экране чека в режиме кассы).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct CashierSettings {
    pub buttons: Vec<ButtonPlace>,
    /// «Провести чек» во всю ширину, «Отложить» и «Очистить» — узким рядом над ним.
    pub submit_wide: bool,
    /// «Выйти из режима кассы» — серой кнопкой, а не синей.
    pub exit_grey: bool,
    /// Высота плиток ходового в режиме кассы, доля высоты экрана, %.
    pub tiles_dvh: i32,
    /// Сколько купюр у «Получено» в режиме кассы.
    pub notes: i32,
    /// Быстрые проценты в окне «Скидка».
    pub discount_quick: Vec<i32>,
    /// Скидка на весь чек снижает и цены услуг.
    pub discount_services: bool,
    /// Предел скидки администратора на чек, % от суммы чека по прайсу; пусто — без предела.
    pub admin_limit_pct: Option<i32>,
    /// Предел скидки администратора на чек, тыйын; пусто — без предела.
    pub admin_limit_tyiyn: Option<i64>,
    /// Печатать строку «Скидка» в товарном чеке.
    pub print_discount: bool,
    /// Печатать раздел «Прибыль» в чеке смены (по умолчанию нет, задача 003 Н4).
    pub print_shift_profit: bool,
    /// Себестоимость и прибыль на экране чека на устройстве в режиме кассы (по умолчанию нет).
    pub sale_cost_on_till: bool,
}

impl Default for CashierSettings {
    fn default() -> Self {
        Self {
            buttons: BUTTONS
                .iter()
                .map(|(id, _, place)| ButtonPlace {
                    id: (*id).into(),
                    place: (*place).into(),
                })
                .collect(),
            submit_wide: false,
            exit_grey: false,
            tiles_dvh: 24,
            notes: 1,
            discount_quick: vec![5, 10, 15],
            discount_services: true,
            admin_limit_pct: None,
            admin_limit_tyiyn: None,
            print_discount: false,
            print_shift_profit: false,
            sale_cost_on_till: false,
        }
    }
}

#[derive(Deserialize)]
struct SettingsIn {
    op_id: Uuid,
    #[serde(flatten)]
    settings: CashierSettings,
}

/// Проверка и порядок: известные кнопки без повторов, места из допустимых; недостающие кнопки —
/// на свои места по умолчанию в конце; числа — в пределах.
pub fn normalize(mut s: CashierSettings) -> AppResult<CashierSettings> {
    let mut seen: Vec<&str> = Vec::new();
    for b in &s.buttons {
        let Some((_, places, _)) = BUTTONS.iter().find(|(id, _, _)| *id == b.id) else {
            return Err(invalid(format!("неизвестная кнопка кассы: {}", b.id)));
        };
        if !places.contains(&b.place.as_str()) {
            return Err(invalid(format!(
                "кнопке «{}» нельзя место «{}»",
                b.id, b.place
            )));
        }
        if seen.contains(&b.id.as_str()) {
            return Err(invalid(format!("кнопка {} указана дважды", b.id)));
        }
        seen.push(&b.id);
    }
    let missing: Vec<ButtonPlace> = BUTTONS
        .iter()
        .filter(|(id, _, _)| !s.buttons.iter().any(|b| b.id == *id))
        .map(|(id, _, place)| ButtonPlace {
            id: (*id).into(),
            place: (*place).into(),
        })
        .collect();
    s.buttons.extend(missing);
    if !(15..=50).contains(&s.tiles_dvh) {
        return Err(invalid("высота плиток — от 15 до 50 % экрана"));
    }
    if !(1..=3).contains(&s.notes) {
        return Err(invalid("купюр у «Получено» — от 1 до 3"));
    }
    if s.discount_quick.len() > 5 || s.discount_quick.iter().any(|p| !(1..=99).contains(p)) {
        return Err(invalid("быстрые проценты скидки — до пяти, от 1 до 99"));
    }
    s.discount_quick.sort_unstable();
    s.discount_quick.dedup();
    if s.admin_limit_pct.is_some_and(|p| !(0..=100).contains(&p)) {
        return Err(invalid("предел скидки — от 0 до 100 %"));
    }
    if let Some(t) = s.admin_limit_tyiyn {
        if t < 0 {
            return Err(invalid("предел скидки — не меньше нуля"));
        }
        ops::check_amount(t)?;
    }
    Ok(s)
}

/// Настройка кассы филиала; нет записи или она не читается — по умолчанию.
pub async fn load(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<CashierSettings> {
    let v = sqlx::query_scalar!(
        "select value from settings where branch_id = $1 and key = 'cashier'",
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(
        v.and_then(|v| serde_json::from_value::<CashierSettings>(v).ok())
            .and_then(|s| normalize(s).ok())
            .unwrap_or_default(),
    )
}

async fn get_settings(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<CashierSettings>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(load(&mut conn, user.branch_id).await?))
}

async fn put_settings(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<SettingsIn>,
) -> AppResult<Json<CashierSettings>> {
    // Раскладку, предел скидки и прибыль на бумаге меняет только владелец (I12, I13).
    ctx.user.require_owner()?;
    const KIND: &str = "settings.cashier";
    let settings = normalize(req.settings)?;
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op::<CashierSettings>(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let before = load(&mut tx, ctx.user.branch_id).await?;
    let value = serde_json::to_value(&settings).map_err(|e| AppError::Internal(e.to_string()))?;
    sqlx::query!(
        r#"insert into settings (branch_id, key, value) values ($1, 'cashier', $2)
           on conflict (branch_id, key) do update set value = excluded.value"#,
        ctx.user.branch_id,
        value
    )
    .execute(&mut *tx)
    .await?;
    if before != settings {
        ops::audit(
            &mut tx,
            &ctx,
            KIND,
            "settings",
            None,
            json!({ "before": before, "after": settings }),
        )
        .await?;
    }
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &settings).await?;
    tx.commit().await?;
    Ok(Json(settings))
}

/// Скидка чека и сумма по прайсу, тыйын: чего не хватает до прайса по строкам ниже прайса и
/// сумма всех строк (без подарков) по прайсу. Проверка предела администратора (ADR-147).
pub fn limit_exceeded(s: &CashierSettings, discount: i64, list_total: i64) -> Option<String> {
    let pct = s
        .admin_limit_pct
        .filter(|p| i128::from(discount) * 100 > i128::from(*p) * i128::from(list_total.max(0)));
    let sum = s.admin_limit_tyiyn.filter(|t| discount > *t);
    match (pct, sum) {
        (Some(p), _) => Some(format!("{p} %")),
        (None, Some(t)) => Some(crate::domain::money::format_som(t)),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_current_layout() {
        let d = CashierSettings::default();
        assert_eq!(d.buttons.len(), BUTTONS.len());
        assert!(
            d.buttons
                .iter()
                .filter(|b| b.id != "pay_fix")
                .all(|b| b.place != "more" && b.place != "hidden")
        );
        // Новая кнопка «Наличные ↔ QR» скрыта: «Ещё действия…» по умолчанию не появляется.
        assert!(
            d.buttons
                .iter()
                .any(|b| b.id == "pay_fix" && b.place == "hidden")
        );
        assert!(!d.print_shift_profit);
        assert!(!d.sale_cost_on_till);
        let parsed: CashierSettings = serde_json::from_value(json!({})).unwrap();
        assert_eq!(normalize(parsed).unwrap(), d);
    }

    #[test]
    fn buttons_match_the_screen_list() {
        // Кнопки и места по умолчанию — на сервере и на экране (`web/src/lib/cashierLayout.ts`).
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../web/src/lib/cashierLayout.ts"
        );
        let Ok(ts) = std::fs::read_to_string(path) else {
            // Сборка одного сервера без клиента: сверять не с чем.
            return;
        };
        for (id, _, place) in BUTTONS {
            let at = ts
                .find(&format!("{{ id: '{id}'"))
                .unwrap_or_else(|| panic!("на экране нет кнопки {id}"));
            let entry = &ts[at..at + ts[at..].find('}').unwrap_or(0)];
            assert!(entry.contains(&format!("place: '{place}'")), "{entry}");
        }
        assert_eq!(ts.matches("{ id: '").count(), BUTTONS.len());
    }

    #[test]
    fn buttons_checked_and_completed() {
        let s = normalize(CashierSettings {
            buttons: vec![ButtonPlace {
                id: "bonus".into(),
                place: "total".into(),
            }],
            ..CashierSettings::default()
        })
        .unwrap();
        assert_eq!(s.buttons[0].id, "bonus");
        assert_eq!(s.buttons.len(), BUTTONS.len());
        let bad = |id: &str, place: &str| {
            normalize(CashierSettings {
                buttons: vec![ButtonPlace {
                    id: id.into(),
                    place: place.into(),
                }],
                ..CashierSettings::default()
            })
            .is_err()
        };
        assert!(bad("print_money", "search"));
        assert!(bad("exit", "hidden"));
        assert!(bad("keyboard", "total"));
        let twice = vec![
            ButtonPlace {
                id: "bonus".into(),
                place: "total".into(),
            };
            2
        ];
        assert!(
            normalize(CashierSettings {
                buttons: twice,
                ..CashierSettings::default()
            })
            .is_err()
        );
        assert!(
            normalize(CashierSettings {
                admin_limit_pct: Some(101),
                ..CashierSettings::default()
            })
            .is_err()
        );
    }

    #[test]
    fn admin_limit() {
        let s = CashierSettings {
            admin_limit_pct: Some(10),
            ..CashierSettings::default()
        };
        assert_eq!(limit_exceeded(&s, 1_000, 10_000), None);
        assert_eq!(limit_exceeded(&s, 1_001, 10_000), Some("10 %".into()));
        let s = CashierSettings {
            admin_limit_tyiyn: Some(50_000),
            ..CashierSettings::default()
        };
        assert_eq!(limit_exceeded(&s, 50_000, 1_000_000), None);
        assert!(limit_exceeded(&s, 50_001, 1_000_000).is_some());
        assert_eq!(
            limit_exceeded(&CashierSettings::default(), 9_000, 10_000),
            None
        );
    }
}
