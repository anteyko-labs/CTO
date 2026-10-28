//! Кассы, движения наличных и смена (SPEC-05).

use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::auth::{Ctx, CurrentUser};
use crate::error::{AppError, AppResult, invalid};
use crate::ops::{self, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/cash/accounts",
            routing::get(list_accounts).post(create_account),
        )
        .route(
            "/cash/accounts/{id}",
            routing::get(account_movements).patch(update_account),
        )
        .route("/cash/accounts/{id}/summary", routing::get(account_summary))
        .route("/cash/movements", routing::post(post_movement))
        .route(
            "/cash/movements/{id}/reverse",
            routing::post(reverse_movement),
        )
        .route("/cash/transfers", routing::post(post_transfer))
        .route(
            "/cash/transfers/{id}/reverse",
            routing::post(reverse_transfer),
        )
        .route("/shifts", routing::get(list_shifts))
        .route("/shifts/open", routing::post(open_shift))
        .route("/shifts/current", routing::get(current_shift))
        .route("/shifts/handover-target", routing::get(handover_target))
        .route("/shifts/{id}", routing::get(get_shift))
        .route("/shifts/{id}/close", routing::post(close_shift))
        .route("/shifts/{id}/reopen", routing::post(reopen_shift))
        .route("/shifts/{id}/handover", routing::post(handover_shift))
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AccountOut {
    pub id: Uuid,
    pub name: String,
    /// `register` — касса смены, `safe`, `other` — наличные без смены, `bank` — безналичный счёт.
    pub kind: String,
    pub owner_only: bool,
    /// Основная касса смены (`register`); у безналичных счетов — всегда `false` (ADR-077).
    pub is_default: bool,
    /// Безналичный счёт по умолчанию для оплаты картой (ADR-077).
    #[serde(default)]
    pub default_card: bool,
    /// Безналичный счёт по умолчанию для оплаты переводом (ADR-077).
    #[serde(default)]
    pub default_transfer: bool,
    /// Безналичный счёт по умолчанию для оплаты «По счёту» (ADR-095); ни у одного — счёт карты.
    #[serde(default)]
    pub default_invoice: bool,
    /// Ставка комиссии банка на сегодня, сотые доли процента (ADR-077); у наличных — 0.
    #[serde(default)]
    pub fee_rate_bp: i32,
    /// Можно уйти ниже нуля (ADR-076).
    #[serde(default)]
    pub allow_negative: bool,
    /// В архиве — `false`: не предлагается к выбору, история остаётся.
    #[serde(default = "yes")]
    pub active: bool,
    pub balance_tyiyn: i64,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
struct AccountsQuery {
    /// Вместе с архивными — экран «Обзор счетов».
    #[serde(default)]
    archived: bool,
}

async fn list_accounts(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<AccountsQuery>,
) -> AppResult<Json<Vec<AccountOut>>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(accounts_list(&mut conn, &user, q.archived).await?))
}

/// Кассы с остатками — экран «Деньги» и бот владельца (SPEC-24): только действующие.
pub(crate) async fn accounts_data(
    conn: &mut PgConnection,
    user: &CurrentUser,
) -> AppResult<Vec<AccountOut>> {
    accounts_list(conn, user, false).await
}

/// Кассы и счета филиала; с `archived` — и архивные («Обзор счетов», SPEC-28 §5).
pub(crate) async fn accounts_list(
    conn: &mut PgConnection,
    user: &CurrentUser,
    archived: bool,
) -> AppResult<Vec<AccountOut>> {
    ensure_accounts(&mut *conn, user.branch_id).await?;
    // Сейф владельца администратору не показываем, перевести в него он может (SPEC-05).
    let rows = sqlx::query_as!(
        AccountOut,
        r#"select id, name, kind, owner_only, is_default, default_card, default_transfer, default_invoice, allow_negative,
                  active, balance_tyiyn,
                  account_fee_on(id, (now() at time zone 'Asia/Bishkek')::date) as "fee_rate_bp!"
           from cash_accounts
           where branch_id = $1 and (active or $3) and (not owner_only or $2)
           order by active desc, kind = 'bank', is_default desc, default_card desc,
                    default_transfer desc, kind, name"#,
        user.branch_id,
        user.is_owner(),
        archived
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows)
}

/// Новый филиал получает свои кассы при первом обращении: касса, счёт и два сейфа (ADR-037).
/// «Касса» — касса смены, «Счёт» — счёт по умолчанию для карты и перевода со ставкой 0,5 %
/// (ADR-022, ADR-077).
pub async fn ensure_accounts(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<()> {
    let have = sqlx::query_scalar!(
        r#"select count(*) as "n!" from cash_accounts where branch_id = $1"#,
        branch_id
    )
    .fetch_one(&mut *conn)
    .await?;
    if have > 0 {
        return Ok(());
    }
    for (name, kind, owner_only, is_default) in [
        ("Касса", "register", false, true),
        ("Счёт", "bank", false, false),
        ("Сейф магазина", "safe", false, false),
        ("Сейф владельца", "safe", true, false),
    ] {
        let bank = kind == "bank";
        sqlx::query!(
            r#"insert into cash_accounts (id, branch_id, name, kind, owner_only, is_default,
                                          default_card, default_transfer)
               values ($1, $2, $3, $4, $5, $6, $7, $7)
               on conflict do nothing"#,
            new_id(),
            branch_id,
            name,
            kind,
            owner_only,
            is_default,
            bank
        )
        .execute(&mut *conn)
        .await?;
    }
    crate::api::fees::ensure_default_fee(conn, branch_id).await
}

/// Касса по умолчанию: в неё идут наличные чека, к ней привязана смена.
pub async fn default_account(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<Uuid> {
    ensure_accounts(conn, branch_id).await?;
    sqlx::query_scalar!(
        r#"select id from cash_accounts
           where branch_id = $1 and kind = 'register' and is_default and active
           order by created_at limit 1"#,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| AppError::Internal("нет кассы по умолчанию".into()))
}

/// Безналичный счёт по умолчанию для способа оплаты: карта — отмеченный «по умолчанию для
/// карты», перевод — «по умолчанию для перевода» (ADR-077), «По счёту» — «по умолчанию для
/// «По счёту»», а без такой отметки — счёт карты (ADR-095). Без отметки — первый действующий.
pub async fn bank_account(
    conn: &mut PgConnection,
    branch_id: Uuid,
    method: &str,
) -> AppResult<Option<Uuid>> {
    ensure_accounts(conn, branch_id).await?;
    Ok(sqlx::query_scalar!(
        r#"select id from cash_accounts where branch_id = $1 and kind = 'bank' and active
           order by (case $2 when 'transfer' then default_transfer
                             when 'invoice' then default_invoice
                             else default_card end) desc,
                    default_card desc, created_at limit 1"#,
        branch_id,
        method
    )
    .fetch_optional(&mut *conn)
    .await?)
}

/// Счёт оплаты картой или переводом (ADR-076, ADR-077): выбранный кассиром безналичный счёт или
/// счёт по умолчанию для этого способа. Чек без сети уже состоялся: счёт, который за это время
/// убрали в архив или которого нет, не отклоняет его — деньги ложатся на счёт по умолчанию,
/// причина уходит владельцу.
pub async fn sale_bank_account(
    conn: &mut PgConnection,
    user: &CurrentUser,
    method: &str,
    requested: Option<Uuid>,
    offline: bool,
    bypass: &mut Vec<String>,
) -> AppResult<Uuid> {
    let default = bank_account(&mut *conn, user.branch_id, method)
        .await?
        .ok_or_else(|| invalid("нет безналичного счёта: заведите его в «Обзоре счетов»"))?;
    let Some(id) = requested else {
        return Ok(default);
    };
    let acc = sqlx::query!(
        "select name, kind, owner_only, active from cash_accounts where id = $1 and branch_id = $2",
        id,
        user.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    let problem = match &acc {
        None => Some("выбранный счёт не найден".to_string()),
        Some(a) if a.kind != "bank" => Some(format!("«{}» — не безналичный счёт", a.name)),
        Some(a) if a.owner_only && !user.is_owner() => Some("выбранный счёт не найден".into()),
        Some(a) if !a.active => Some(format!("счёт «{}» в архиве", a.name)),
        Some(_) => None,
    };
    let Some(problem) = problem else {
        return Ok(id);
    };
    if !offline {
        return Err(invalid(format!("{problem}: выберите другой счёт")));
    }
    let name = sqlx::query_scalar!("select name from cash_accounts where id = $1", default)
        .fetch_one(&mut *conn)
        .await?;
    bypass.push(format!("{problem} — оплата записана на «{name}»"));
    Ok(default)
}

/// Касса, из которой платят (расход, выплата, накладная, внесение и изъятие): из этого
/// филиала и не в архиве; администратор — только из основной кассы смены, кассы владельца —
/// только владелец (ADR-037, ADR-076). Возвращает вид кассы.
pub async fn check_pay_account(
    conn: &mut PgConnection,
    user: &CurrentUser,
    id: Uuid,
) -> AppResult<String> {
    let acc = sqlx::query!(
        "select kind, owner_only, active, is_default from cash_accounts where id = $1 and branch_id = $2",
        id,
        user.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| invalid("касса не найдена"))?;
    if acc.owner_only && !user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if !user.is_owner() && !(acc.kind == "register" && acc.is_default) {
        return Err(AppError::Forbidden);
    }
    if !acc.active {
        return Err(invalid("касса в архиве"));
    }
    Ok(acc.kind)
}

pub struct CashEntry<'a> {
    pub account_id: Uuid,
    pub kind: &'a str,
    pub amount: i64,
    pub doc_type: &'a str,
    pub doc_id: Option<Uuid>,
    pub comment: &'a str,
}

/// Движение денег, записанное не сразу: «Изменить оплату» (SPEC-37 §1) пишет деньги сторно и
/// нового чека в конце операции — сначала с плюсом, потом с минусом, чтобы касса не уходила в минус
/// даже на миг.
#[derive(Clone, Debug)]
pub struct CashMove {
    pub account_id: Uuid,
    pub kind: &'static str,
    pub amount: i64,
    pub doc_type: &'static str,
    pub doc_id: Option<Uuid>,
    pub comment: String,
}

impl CashMove {
    pub fn entry(&self) -> CashEntry<'_> {
        CashEntry {
            account_id: self.account_id,
            kind: self.kind,
            amount: self.amount,
            doc_type: self.doc_type,
            doc_id: self.doc_id,
            comment: &self.comment,
        }
    }
}

/// Записывает движение денег и пересчитывает остаток кассы (инвариант 6).
/// Смена подставляется сама: открытая на этой кассе в момент проведения.
pub async fn add_movement(conn: &mut PgConnection, ctx: &Ctx, e: CashEntry<'_>) -> AppResult<i64> {
    add_movement_id(conn, ctx, e).await.map(|(_, b)| b)
}

/// То же, что [`add_movement`], и возвращает id движения: «Исправить» связывает внесение или
/// изъятие с исправленным (SPEC-29 §24).
pub async fn add_movement_id(
    conn: &mut PgConnection,
    ctx: &Ctx,
    e: CashEntry<'_>,
) -> AppResult<(Uuid, i64)> {
    write_movement(conn, ctx, e, None).await
}

/// То же, что [`add_movement_id`], но смена движения — заданная, а не открытая сейчас: деньги
/// изменения оплаты для уже пересчитанных наличных относятся к смене пересчёта (SPEC-37 §4).
pub async fn add_movement_in_shift(
    conn: &mut PgConnection,
    ctx: &Ctx,
    e: CashEntry<'_>,
    shift_id: Uuid,
) -> AppResult<(Uuid, i64)> {
    write_movement(conn, ctx, e, Some(shift_id)).await
}

async fn write_movement(
    conn: &mut PgConnection,
    ctx: &Ctx,
    e: CashEntry<'_>,
    shift: Option<Uuid>,
) -> AppResult<(Uuid, i64)> {
    // Сумма каждой операции проверена на её входе; здесь — предел остатка кассы.
    ops::check_balance(e.amount)?;
    let acc = sqlx::query!(
        "select name, balance_tyiyn, allow_negative, active from cash_accounts where id = $1 and branch_id = $2 for update",
        e.account_id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| invalid("касса не найдена"))?;
    // Счёт в архиве денег не принимает и не отдаёт — и сторно тоже: иначе остаток
    // «невидимого» счёта разошёлся бы с итогами (ADR-088). Вернуть из архива можно в «Счетах».
    if !acc.active {
        return Err(AppError::Conflict(format!(
            "счёт «{}» в архиве — верните его из архива в «Счетах» и повторите",
            acc.name
        )));
    }
    let balance = acc.balance_tyiyn;
    let next = balance
        .checked_add(e.amount)
        .ok_or_else(|| AppError::Validation("слишком большая сумма".into()))?;
    // Любой расход проверяется здесь, в одном месте: ниже нуля — только если у счёта это
    // разрешено (ADR-076; по умолчанию разрешено у всех — ADR-077). Поступление не отклоняется,
    // даже если остаток всё ещё меньше нуля.
    if next < 0 && e.amount < 0 && !acc.allow_negative {
        return Err(invalid(format!(
            "на счёте «{}» только {}",
            acc.name,
            crate::domain::money::format_som(balance)
        )));
    }
    ops::check_balance(next)?;
    // Счёт ушёл в минус — владельцу один раз на каждый переход через ноль (SPEC-13, ADR-077).
    if balance >= 0 && next < 0 {
        let doc = doc_label(&mut *conn, e.kind, e.doc_type, e.doc_id).await?;
        ops::audit(
            &mut *conn,
            ctx,
            "cash.negative",
            "cash_account",
            Some(e.account_id),
            json!({
                "account": acc.name,
                "balance": next,
                "amount": e.amount,
                "doc": doc,
                "doc_type": e.doc_type,
                "doc_id": e.doc_id,
            }),
        )
        .await?;
    }
    let shift_id = match shift {
        Some(s) => Some(s),
        None => open_shift_id(conn, ctx.user.branch_id, e.account_id).await?,
    };
    let movement_id = new_id();
    sqlx::query!(
        r#"insert into cash_movements (id, branch_id, account_id, shift_id, kind, amount_tyiyn,
                                       doc_type, doc_id, comment, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)"#,
        movement_id,
        ctx.user.branch_id,
        e.account_id,
        shift_id,
        e.kind,
        e.amount,
        e.doc_type,
        e.doc_id,
        e.comment,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await?;
    sqlx::query!(
        "update cash_accounts set balance_tyiyn = $2 where id = $1",
        e.account_id,
        next
    )
    .execute(&mut *conn)
    .await?;
    Ok((movement_id, next))
}

/// Документ движения словами для уведомления: «расход № 15», «чек № 42», «изъятие».
async fn doc_label(
    conn: &mut PgConnection,
    kind: &str,
    doc_type: &str,
    doc_id: Option<Uuid>,
) -> AppResult<String> {
    let number = match doc_id {
        None => None,
        Some(id) => sqlx::query_scalar!(
            r#"select case $1
                 when 'expense' then (select number from expenses where id = $2)::bigint
                 when 'sale' then (select number from sales where id = $2)::bigint
                 when 'receipt' then (select number from receipts where id = $2)::bigint
                 when 'payout' then (select number from payouts where id = $2)::bigint
                 when 'transfer' then (select number from cash_transfers where id = $2)::bigint
                 when 'shift' then (select number from shifts where id = $2)::bigint
                 when 'battery_intake' then (select number from battery_intakes where id = $2)::bigint
                 when 'defect' then (select number from defects where id = $2)::bigint
               end as "n?""#,
            doc_type,
            id
        )
        .fetch_one(&mut *conn)
        .await?,
    };
    let word = match (doc_type, kind) {
        (_, "bank_fee") => "комиссия банка",
        (_, "reversal") => "сторно",
        ("pay_fix", _) | (_, "cash_to_bank") => "перенос из наличных в QR",
        ("expense", _) => "расход",
        ("sale", "sale_return") => "возврат по чеку",
        ("sale", _) => "чек",
        ("receipt", _) => "оплата накладной",
        ("payout", _) => "выплата",
        ("transfer", _) => "перемещение",
        ("shift", _) => "пересчёт смены",
        ("battery_intake", _) => "приём аккумуляторов",
        ("defect", _) => "возврат денег поставщику",
        ("repayment", _) => "оплата долга",
        (_, "cash_out") => "изъятие",
        (_, "cash_in") => "внесение",
        _ => "движение денег",
    };
    Ok(match number {
        Some(n) => format!("{word} № {n}"),
        None => word.to_string(),
    })
}

/// Открытая смена кассы: последнее закрытие переоткрыто или закрытий нет.
pub async fn open_shift_id(
    conn: &mut PgConnection,
    branch_id: Uuid,
    account_id: Uuid,
) -> AppResult<Option<Uuid>> {
    Ok(sqlx::query_scalar!(
        r#"select s.id from shifts s
           where s.branch_id = $1 and s.account_id = $2
             and not exists (
               select 1 from shift_closes c
               where c.shift_id = s.id
                 and not exists (select 1 from shift_reopens r where r.close_id = c.id))
           order by s.opened_at desc limit 1"#,
        branch_id,
        account_id
    )
    .fetch_optional(&mut *conn)
    .await?)
}

/// Смена кассы, открытая в момент `at` (без времени — сейчас): с учётом закрытий и
/// переоткрытий до этого момента. Кассир чека — кассир этой смены (ADR-071).
pub struct ShiftAt {
    pub id: Uuid,
    pub number: i64,
    pub cashier_employee_id: Uuid,
}

pub async fn shift_open_at(
    conn: &mut PgConnection,
    branch_id: Uuid,
    account_id: Uuid,
    at: Option<DateTime<Utc>>,
) -> AppResult<Option<ShiftAt>> {
    Ok(sqlx::query_as!(
        ShiftAt,
        r#"select s.id, s.number, s.cashier_employee_id from shifts s
           where s.branch_id = $1 and s.account_id = $2
             and s.opened_at <= coalesce($3, now())
             and not exists (
               select 1 from shift_closes c
               where c.shift_id = s.id and c.closed_at <= coalesce($3, now())
                 and not exists (select 1 from shift_reopens r
                                 where r.close_id = c.id and r.created_at <= coalesce($3, now())))
           order by s.opened_at desc limit 1"#,
        branch_id,
        account_id,
        at
    )
    .fetch_optional(&mut *conn)
    .await?)
}

/// Деньги через кассу с наличными — только в открытой смене: иначе их не с чем сверить
/// при закрытии (SPEC-05, SPEC-06, SPEC-07). Продажи и сдача кассы этим не ограничены (ADR-021).
pub async fn require_open_shift(
    conn: &mut PgConnection,
    branch_id: Uuid,
    account_id: Uuid,
) -> AppResult<()> {
    let kind = sqlx::query_scalar!(
        "select kind from cash_accounts where id = $1 and branch_id = $2",
        account_id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| invalid("касса не найдена"))?;
    if kind == "register" && open_shift_id(conn, branch_id, account_id).await?.is_none() {
        return Err(AppError::Conflict(
            "смена не открыта: откройте её на экране «Смена»".into(),
        ));
    }
    Ok(())
}

/// Блокирует строки касс в порядке id: два встречных перемещения не ждут друг друга вечно.
async fn lock_accounts(conn: &mut PgConnection, branch_id: Uuid, ids: &[Uuid]) -> AppResult<()> {
    let mut ids = ids.to_vec();
    ids.sort();
    ids.dedup();
    let locked = sqlx::query_scalar!(
        "select id from cash_accounts where branch_id = $1 and id = any($2) order by id for update",
        branch_id,
        &ids
    )
    .fetch_all(&mut *conn)
    .await?;
    if locked.len() != ids.len() {
        return Err(invalid("касса не найдена"));
    }
    Ok(())
}

#[derive(Serialize)]
struct BreakdownRow {
    kind: String,
    sum_tyiyn: i64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ShiftOut {
    pub id: Uuid,
    pub number: i64,
    pub business_date: NaiveDate,
    pub account_id: Uuid,
    pub account_name: String,
    pub cashier_name: String,
    #[serde(default)]
    pub cashier_employee_id: Option<Uuid>,
    pub opened_by: String,
    pub opened_at: DateTime<Utc>,
    pub opening_expected_tyiyn: i64,
    /// Открытая смена — сколько должно быть в кассе сейчас; закрытая — сколько ждали при закрытии.
    pub expected_tyiyn: i64,
    pub closed_at: Option<DateTime<Utc>>,
    /// Кто закрыл смену (действующее закрытие) — для чека смены (SPEC-36 §10).
    #[serde(default)]
    pub closed_by: Option<String>,
    pub counted_tyiyn: Option<i64>,
    pub diff_tyiyn: Option<i64>,
    /// Поправки расхождения закрытия после закрытия (SPEC-37 §4): изменения оплаты и переносы.
    /// Расхождение сейчас — `diff_tyiyn` плюс это; ноль — поправок не было.
    #[serde(default)]
    pub diff_fix_tyiyn: i64,
    #[serde(default)]
    pub has_after_close: bool,
    pub breakdown: Value,
    pub cash_sales_tyiyn: i64,
    pub card_tyiyn: i64,
    pub transfer_tyiyn: i64,
    /// «По счёту» (ADR-095): юрлица перечислением.
    #[serde(default)]
    pub invoice_tyiyn: i64,
    pub debt_tyiyn: i64,
    #[serde(default)]
    pub bonus_tyiyn: i64,
    /// Принято аккумуляторами в оплату чеков смены (SPEC-27).
    #[serde(default)]
    pub battery_tyiyn: i64,
    #[serde(default)]
    pub bank_fee_tyiyn: i64,
    /// Безнал смены по безналичным счетам: куда пришли карта и перевод (ADR-076).
    #[serde(default)]
    pub bank_split: Vec<crate::api::income::BankSplit>,
    /// Смена закрыта, а что сделали с деньгами (сейф или размен) ещё не отмечено.
    #[serde(default)]
    pub handover_pending: bool,
    #[serde(default)]
    pub to_safe_tyiyn: Option<i64>,
    #[serde(default)]
    pub left_tyiyn: Option<i64>,
    /// Чеки смены: сколько, на какую сумму, возвраты.
    #[serde(default)]
    pub sales_count: i64,
    #[serde(default)]
    pub sales_total_tyiyn: i64,
    #[serde(default)]
    pub returns_tyiyn: i64,
    /// Кто сколько заработал в день смены.
    #[serde(default)]
    pub pay: Vec<crate::api::payroll::StaffPay>,
}

pub(crate) async fn load_shift(
    conn: &mut PgConnection,
    branch_id: Uuid,
    id: Uuid,
) -> AppResult<ShiftOut> {
    load_shift_with(conn, branch_id, id, &mut HashMap::new()).await
}

/// Смена; `owed` — долг по работникам, общий для всех смен списка (см. `staff_pay_with`).
async fn load_shift_with(
    conn: &mut PgConnection,
    branch_id: Uuid,
    id: Uuid,
    owed: &mut HashMap<Uuid, i64>,
) -> AppResult<ShiftOut> {
    let h = sqlx::query!(
        r#"select s.id, s.number, s.business_date, s.account_id, a.name as account_name,
                  e.full_name as cashier_name, s.cashier_employee_id, u.full_name as opened_by, s.opened_at,
                  s.opening_expected_tyiyn, a.balance_tyiyn
           from shifts s
           join cash_accounts a on a.id = s.account_id
           join employees e on e.id = s.cashier_employee_id
           join users u on u.id = s.opened_by
           where s.id = $1 and s.branch_id = $2"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    let close = sqlx::query!(
        r#"select c.closed_at, c.expected_tyiyn, c.counted_tyiyn, c.diff_tyiyn,
                  h.to_safe_tyiyn as "to_safe?", h.left_tyiyn as "left?",
                  (select u.full_name from users u where u.id = c.user_id) as "closed_by?"
           from shift_closes c
           left join shift_handovers h on h.close_id = c.id
             and (h.transfer_id is null or not exists (select 1 from cash_transfers x where x.reversal_of = h.transfer_id))
           where c.shift_id = $1
             and not exists (select 1 from shift_reopens r where r.close_id = c.id)
           order by c.seq desc limit 1"#,
        id
    )
    .fetch_optional(&mut *conn)
    .await?;
    // Разбивка наличных: касса смены за время смены, без счёта и сейфов. По времени, а не по
    // отметке смены: перевод владельца или чек без сети тоже двигают деньги в этой кассе,
    // и «на начало + движения» должно сходиться с «должно быть». Пересчёт при закрытии
    // (то же время, что закрытие) не входит: «должно быть» считали до него. Выплаты при
    // закрытии (та же операция, то же время) входят: «должно быть» считали после них.
    let breakdown: Vec<BreakdownRow> = sqlx::query!(
        r#"select kind as "kind!", sum(amount_tyiyn)::bigint as "sum!"
           from cash_movements
           where account_id = $1 and created_at >= $2
             and (created_at < coalesce($3, 'infinity'::timestamptz)
                  or (created_at = $3 and kind <> 'count_diff'))
             and not public.pay_fix_counted_move(id, doc_type, doc_id)
           group by kind order by kind"#,
        h.account_id,
        h.opened_at,
        close.as_ref().map(|c| c.closed_at)
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| BreakdownRow {
        kind: r.kind,
        sum_tyiyn: r.sum,
    })
    .collect();
    // Сверка с терминалом и банком: чеки, проведённые, пока смена была открыта.
    // Берём по времени, а не по движениям кассы: чек картой или в долг наличных не трогает.
    let pays = sqlx::query!(
        r#"select
             coalesce(sum(p.amount_tyiyn) filter (where p.method = 'cash'), 0)::bigint as "cash!",
             coalesce(sum(p.amount_tyiyn) filter (where p.method = 'card'), 0)::bigint as "card!",
             coalesce(sum(p.amount_tyiyn) filter (where p.method = 'transfer'), 0)::bigint as "transfer!",
             coalesce(sum(p.amount_tyiyn) filter (where p.method = 'invoice'), 0)::bigint as "invoice!",
             coalesce(sum(p.amount_tyiyn) filter (where p.method = 'debt'), 0)::bigint as "debt!",
             coalesce(sum(p.amount_tyiyn) filter (where p.method = 'bonus'), 0)::bigint as "bonus!",
             coalesce(sum(p.amount_tyiyn) filter (where p.method = 'battery'), 0)::bigint as "battery!",
             coalesce(sum(p.fee_tyiyn), 0)::bigint as "fee!"
           from sale_payments p
           join sales s on s.id = p.sale_id
           where s.branch_id = $1 and s.created_at >= $2 and s.created_at <= coalesce($3, now())
             and not public.pay_fix_counted_sale(s.id)"#,
        branch_id,
        h.opened_at,
        close.as_ref().map(|c| c.closed_at)
    )
    .fetch_one(&mut *conn)
    .await?;
    let checks = sqlx::query!(
        r#"select count(*) filter (where kind = 'sale') as "count!",
                  coalesce(sum(total_tyiyn) filter (where kind = 'sale'), 0)::bigint as "total!",
                  coalesce(-sum(total_tyiyn) filter (where kind = 'return'), 0)::bigint as "returns!"
           from sales
           where branch_id = $1 and created_at >= $2 and created_at <= coalesce($3, now())
             and not public.pay_fix_counted_sale(id)"#,
        branch_id,
        h.opened_at,
        close.as_ref().map(|c| c.closed_at)
    )
    .fetch_one(&mut *conn)
    .await?;
    let bank_split = crate::api::income::bank_split(
        conn,
        branch_id,
        None,
        Some(h.opened_at),
        close.as_ref().map(|c| c.closed_at),
    )
    .await?;
    let fix = sqlx::query!(
        r#"select count(*) as "n!",
                  coalesce(sum(m.amount_tyiyn) filter (where f.counted_at = 'close'), 0)::bigint as "diff!"
           from pay_fixes f left join cash_movements m on m.id = f.count_movement_id
           where f.counted_shift_id = $1"#,
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    let pay = crate::api::payroll::staff_pay_with(
        conn,
        branch_id,
        h.business_date,
        h.business_date,
        owed,
    )
    .await?;
    Ok(ShiftOut {
        sales_count: checks.count,
        sales_total_tyiyn: checks.total,
        returns_tyiyn: checks.returns,
        pay,
        id: h.id,
        number: h.number,
        business_date: h.business_date,
        account_id: h.account_id,
        account_name: h.account_name,
        cashier_name: h.cashier_name,
        cashier_employee_id: Some(h.cashier_employee_id),
        opened_by: h.opened_by,
        opened_at: h.opened_at,
        opening_expected_tyiyn: h.opening_expected_tyiyn,
        expected_tyiyn: close.as_ref().map_or(h.balance_tyiyn, |c| c.expected_tyiyn),
        closed_at: close.as_ref().map(|c| c.closed_at),
        closed_by: close.as_ref().and_then(|c| c.closed_by.clone()),
        counted_tyiyn: close.as_ref().map(|c| c.counted_tyiyn),
        diff_tyiyn: close.as_ref().map(|c| c.diff_tyiyn),
        diff_fix_tyiyn: fix.diff,
        has_after_close: fix.n > 0,
        breakdown: json!(breakdown),
        cash_sales_tyiyn: pays.cash,
        card_tyiyn: pays.card,
        transfer_tyiyn: pays.transfer,
        invoice_tyiyn: pays.invoice,
        debt_tyiyn: pays.debt,
        bonus_tyiyn: pays.bonus,
        battery_tyiyn: pays.battery,
        bank_fee_tyiyn: pays.fee,
        bank_split,
        handover_pending: close.as_ref().is_some_and(|c| c.to_safe.is_none()),
        to_safe_tyiyn: close.as_ref().and_then(|c| c.to_safe),
        left_tyiyn: close.as_ref().and_then(|c| c.left),
    })
}

async fn current_shift(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<Option<ShiftOut>>> {
    let mut conn = state.pool.acquire().await?;
    let account = default_account(&mut conn, user.branch_id).await?;
    match open_shift_id(&mut conn, user.branch_id, account).await? {
        Some(id) => Ok(Json(Some(load_shift(&mut conn, user.branch_id, id).await?))),
        None => Ok(Json(None)),
    }
}

async fn get_shift(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<ShiftOut>> {
    let mut conn = state.pool.acquire().await?;
    let shift = load_shift(&mut conn, user.branch_id, id).await?;
    // Администратору — те же последние 7 дней, что и в списке смен (SPEC-05); открытая смена
    // видна всегда — с ней работают на кассе.
    if !user.is_owner() && shift.closed_at.is_some() {
        let earliest = sqlx::query_scalar!(
            r#"select ((now() at time zone 'Asia/Bishkek')::date - 6) as "d!""#
        )
        .fetch_one(&mut *conn)
        .await?;
        if shift.business_date < earliest {
            return Err(AppError::NotFound);
        }
    }
    Ok(Json(shift))
}

#[derive(Deserialize)]
struct PeriodQuery {
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
}

async fn list_shifts(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<PeriodQuery>,
) -> AppResult<Json<Vec<ShiftOut>>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(shifts_data(&mut conn, &user, q.from, q.to).await?))
}

/// Смены за период с закрытием — экран «Смены» и бот владельца (SPEC-24).
pub(crate) async fn shifts_data(
    conn: &mut PgConnection,
    user: &CurrentUser,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
) -> AppResult<Vec<ShiftOut>> {
    let q = PeriodQuery { from, to };
    // Администратору — только последние 7 дней (SPEC-05, права).
    let earliest = if user.is_owner() {
        None
    } else {
        Some(
            sqlx::query_scalar!(
                r#"select ((now() at time zone 'Asia/Bishkek')::date - 6) as "d!""#
            )
            .fetch_one(&mut *conn)
            .await?,
        )
    };
    let from = match (q.from, earliest) {
        (Some(f), Some(e)) => Some(f.max(e)),
        (f, e) => f.or(e),
    };
    let ids = sqlx::query_scalar!(
        r#"select id from shifts
           where branch_id = $1
             and ($2::date is null or business_date >= $2)
             and ($3::date is null or business_date <= $3)
           order by business_date desc, opened_at desc limit 60"#,
        user.branch_id,
        from,
        q.to
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut out = Vec::with_capacity(ids.len());
    let mut owed = HashMap::new();
    for id in ids {
        out.push(load_shift_with(&mut *conn, user.branch_id, id, &mut owed).await?);
    }
    Ok(out)
}

#[derive(Deserialize)]
struct OpenReq {
    op_id: Uuid,
    account_id: Option<Uuid>,
    cashier_employee_id: Uuid,
    counted_tyiyn: Option<i64>,
    /// PIN кассира смены (ADR-081).
    #[serde(default)]
    cashier_pin: Option<String>,
    /// Вместо PIN кассира — пароль владельца, который открывает смену сам (ADR-081).
    #[serde(default)]
    owner_password: Option<String>,
}

/// Пароль владельца, который вошёл на этом устройстве (ADR-081, ADR-122): открытие смены за
/// кассира и выплата кассиру при закрытии. Администратору — 403 `not_owner`. Проверка — до
/// транзакции: неверная попытка записывается и тогда, когда операция не прошла; пять неверных
/// за 15 минут — блокировка на 15 минут.
async fn verify_owner_password(
    state: &AppState,
    ctx: &Ctx,
    password: &str,
    not_owner: &str,
) -> AppResult<()> {
    use crate::api::auth::{MAX_FAILURES, clear_attempts, reserve_with_limit};
    if !ctx.user.is_owner() {
        return Err(AppError::ForbiddenWith(not_owner.into()));
    }
    if password.len() > 200 {
        return Err(invalid("неверный пароль владельца"));
    }
    let key = format!("shift-owner|{}", ctx.user.id);
    if let Some(minutes) = reserve_with_limit(state, &key, MAX_FAILURES).await? {
        return Err(AppError::TooManyRequests(format!(
            "слишком много неверных паролей, повторите через {minutes} мин"
        )));
    }
    let hash = sqlx::query_scalar!("select password_hash from users where id = $1", ctx.user.id)
        .fetch_one(&state.pool)
        .await?;
    if !crate::auth::verify_password(password.to_string(), hash).await {
        return Err(invalid("неверный пароль владельца"));
    }
    clear_attempts(state, &key).await
}

/// Открытие смены подтверждает кассир своим PIN или владелец своим паролем (ADR-081).
/// Проверка — до транзакции: неверная попытка записывается и тогда, когда смену не открыли.
/// Пять неверных подряд за 15 минут — блокировка на 15 минут (как у входа, ADR-016).
async fn confirm_opener(state: &AppState, ctx: &Ctx, req: &OpenReq) -> AppResult<&'static str> {
    if let Some(password) = req.owner_password.as_deref().filter(|p| !p.is_empty()) {
        verify_owner_password(
            state,
            ctx,
            password,
            "паролем смену открывает только владелец; кассир вводит свой PIN",
        )
        .await?;
        return Ok("owner_password");
    }
    // Та же проверка и те же пределы — у приёмки товара на планшете (SPEC-39 §4).
    crate::api::auth::check_cashier_pin(
        state,
        ctx,
        crate::api::auth::CashierPin {
            employee_id: req.cashier_employee_id,
            pin: req.cashier_pin.as_deref().unwrap_or(""),
            no_pin_hint: if ctx.user.is_owner() {
                " (или откройте смену своим паролем)"
            } else {
                ""
            },
            acceptance: false,
        },
    )
    .await?;
    Ok("pin")
}

async fn open_shift(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<OpenReq>,
) -> AppResult<Json<ShiftOut>> {
    const KIND: &str = "shift.open";
    let branch_id = ctx.user.branch_id;
    if req.counted_tyiyn.is_some_and(|c| c < 0) {
        return Err(invalid("сумма не может быть меньше нуля"));
    }
    if let Some(c) = req.counted_tyiyn {
        ops::check_balance(c)?;
    }
    let confirmed = confirm_opener(&state, &ctx, &req).await?;
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let account_id = match req.account_id {
        Some(id) => id,
        None => default_account(&mut tx, branch_id).await?,
    };
    let acc = sqlx::query!(
        "select kind, balance_tyiyn from cash_accounts where id = $1 and branch_id = $2 for update",
        account_id,
        branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| invalid("касса не найдена"))?;
    if acc.kind != "register" {
        return Err(invalid("смена открывается только на кассе с наличными"));
    }
    if open_shift_id(&mut tx, branch_id, account_id)
        .await?
        .is_some()
    {
        return Err(AppError::Conflict("смена уже открыта".into()));
    }
    let cashier = sqlx::query!(
        "select is_cashier, active from employees where id = $1 and branch_id = $2",
        req.cashier_employee_id,
        branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| invalid("кассир не найден"))?;
    if !cashier.active || !cashier.is_cashier {
        return Err(invalid("сотрудник не кассир или отключён"));
    }
    let today = sqlx::query_scalar!(r#"select (now() at time zone 'Asia/Bishkek')::date as "d!""#)
        .fetch_one(&mut *tx)
        .await?;
    // Смен в день может быть несколько, по очереди (ADR-071): следующий кассир открывает свою
    // со своим пересчётом; учётный день у всех смен дня общий.
    let number = ops::next_counter(&mut tx, branch_id, "shift").await?;
    let id = new_id();
    sqlx::query!(
        r#"insert into shifts (id, branch_id, number, business_date, account_id, cashier_employee_id,
                               opened_by, opened_device, opening_expected_tyiyn, opening_counted_tyiyn)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"#,
        id,
        branch_id,
        number,
        today,
        account_id,
        req.cashier_employee_id,
        ctx.user.id,
        ctx.device_id,
        acc.balance_tyiyn,
        req.counted_tyiyn
    )
    .execute(&mut *tx)
    .await?;
    if let Some(counted) = req.counted_tyiyn {
        let diff = counted
            .checked_sub(acc.balance_tyiyn)
            .ok_or_else(crate::error::overflow)?;
        if diff != 0 {
            add_movement(
                &mut tx,
                &ctx,
                CashEntry {
                    account_id,
                    kind: "count_diff",
                    amount: diff,
                    doc_type: "shift",
                    doc_id: Some(id),
                    comment: "пересчёт при открытии",
                },
            )
            .await?;
        }
    }
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "shift",
        Some(id),
        json!({
            "number": number,
            "expected": acc.balance_tyiyn,
            "counted": req.counted_tyiyn,
            "confirmed": confirmed,
        }),
    )
    .await?;
    let out = load_shift(&mut tx, branch_id, id).await?;
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct CloseReq {
    op_id: Uuid,
    counted_tyiyn: i64,
    #[serde(default)]
    comment: String,
    /// Выплаты отмеченным сотрудникам из кассы смены перед пересчётом (SPEC-27, ADR-071).
    #[serde(default)]
    payouts: Vec<ClosePayout>,
    /// Пароль владельца: выплата кассиру при закрытии только с ним (ADR-122) — кассир смены
    /// не выплачивает сам себе.
    #[serde(default)]
    owner_password: Option<String>,
}

/// Выплата при закрытии: обычная выплата SPEC-07 со своим `op_id` — по видам `lines`
/// (ADR-122) или, как раньше, одной суммой `amount_tyiyn` без вида.
#[derive(Deserialize)]
struct ClosePayout {
    op_id: Uuid,
    employee_id: Uuid,
    #[serde(default)]
    amount_tyiyn: i64,
    #[serde(default)]
    lines: Vec<crate::api::payroll::PayoutLine>,
    /// Откуда выплатить (SPEC-29 §13): без него — касса смены. Владелец — любой действующий
    /// счёт, администратор — только касса смены (ADR-076).
    #[serde(default)]
    account_id: Option<Uuid>,
}

async fn close_shift(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<CloseReq>,
) -> AppResult<Json<ShiftOut>> {
    const KIND: &str = "shift.close";
    let branch_id = ctx.user.branch_id;
    if req.counted_tyiyn < 0 {
        return Err(invalid("сумма не может быть меньше нуля"));
    }
    ops::check_balance(req.counted_tyiyn)?;
    if req.payouts.len() > 100 {
        return Err(invalid("слишком много выплат за раз"));
    }
    // Кассиру при закрытии выплачивает только владелец своим паролем (ADR-122): иначе кассир
    // смены за кассой владельца выдал бы деньги сам себе. Пароль — до транзакции (ADR-081).
    let paid: Vec<Uuid> = req.payouts.iter().map(|p| p.employee_id).collect();
    let to_cashier = !paid.is_empty()
        && sqlx::query_scalar!(
            r#"select exists(select 1 from employees
                             where id = any($1) and branch_id = $2 and is_cashier) as "e!""#,
            &paid,
            branch_id
        )
        .fetch_one(&state.pool)
        .await?;
    if to_cashier {
        match req.owner_password.as_deref().filter(|p| !p.is_empty()) {
            Some(password) => {
                verify_owner_password(&state, &ctx, password, "Кассиру выплачивает владелец")
                    .await?
            }
            None => {
                return Err(AppError::ForbiddenWith(
                    "Кассиру выплачивает владелец".into(),
                ));
            }
        }
    }
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let shift = sqlx::query!(
        "select number, account_id, cashier_employee_id, business_date from shifts where id = $1 and branch_id = $2",
        id,
        branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    // «Выплатить отмеченным и закрыть»: выплаты из кассы смены и закрытие — одна операция.
    // Каждая выплата — обычная выплата со своим op_id и правилами SPEC-07 (администратор
    // выдаёт только заработанное), и «должно быть» считается уже после них.
    let mut paid_out: i64 = 0;
    if !req.payouts.is_empty() {
        let mut seen_emp = std::collections::HashSet::new();
        let mut seen_op = std::collections::HashSet::from([req.op_id]);
        for p in &req.payouts {
            if !seen_emp.insert(p.employee_id) || !seen_op.insert(p.op_id) {
                return Err(invalid("выплата одному сотруднику — одной строкой"));
            }
        }
        if open_shift_id(&mut tx, branch_id, shift.account_id).await? != Some(id) {
            return Err(AppError::Conflict("смена уже закрыта".into()));
        }
        let comment = format!("при закрытии смены № {}", shift.number);
        for p in &req.payouts {
            let from = p.account_id.unwrap_or(shift.account_id);
            // По видам (ADR-122): те же правила, что у выплаты в «Расчёте».
            let out = crate::api::payroll::post_payouts_tx(
                &mut tx,
                &ctx,
                crate::api::payroll::PayoutsReq {
                    base: crate::api::payroll::PayoutReq {
                        op_id: p.op_id,
                        employee_id: p.employee_id,
                        amount_tyiyn: p.amount_tyiyn,
                        source: Some("account".into()),
                        account_id: Some(from),
                        advance: false,
                        comment: comment.clone(),
                    },
                    lines: p.lines.clone(),
                    purpose: None,
                },
            )
            .await?;
            // «Должно быть» уменьшают только выплаты из кассы смены.
            if from == shift.account_id {
                for l in &out.lines {
                    paid_out = paid_out
                        .checked_add(l.amount_tyiyn)
                        .ok_or_else(crate::error::overflow)?;
                }
            }
        }
    }
    // Сначала блокировка кассы, потом проверка: два устройства не закроют смену дважды.
    let expected = sqlx::query_scalar!(
        "select balance_tyiyn from cash_accounts where id = $1 for update",
        shift.account_id
    )
    .fetch_one(&mut *tx)
    .await?;
    if open_shift_id(&mut tx, branch_id, shift.account_id).await? != Some(id) {
        return Err(AppError::Conflict("смена уже закрыта".into()));
    }
    let diff = req
        .counted_tyiyn
        .checked_sub(expected)
        .ok_or_else(crate::error::overflow)?;
    if diff != 0 && req.comment.trim().is_empty() {
        return Err(invalid("при расхождении нужен комментарий"));
    }
    if diff != 0 {
        add_movement(
            &mut tx,
            &ctx,
            CashEntry {
                account_id: shift.account_id,
                kind: "count_diff",
                amount: diff,
                doc_type: "shift",
                doc_id: Some(id),
                comment: req.comment.trim(),
            },
        )
        .await?;
    }
    crate::api::payroll::accrue_for_shift_close(
        &mut tx,
        &ctx,
        id,
        shift.business_date,
        shift.cashier_employee_id,
        diff,
    )
    .await?;
    let seq = sqlx::query_scalar!(
        r#"select coalesce(max(seq), 0) + 1 as "n!" from shift_closes where shift_id = $1"#,
        id
    )
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query!(
        r#"insert into shift_closes (id, shift_id, seq, expected_tyiyn, counted_tyiyn, diff_tyiyn,
                                     comment, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
        new_id(),
        id,
        seq,
        expected,
        req.counted_tyiyn,
        diff,
        req.comment.trim(),
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "shift",
        Some(id),
        json!({
            "expected": expected,
            "counted": req.counted_tyiyn,
            "diff": diff,
            "payouts": req.payouts.len(),
            "paid_out": paid_out,
        }),
    )
    .await?;
    // Повторное закрытие после переоткрытия с излишком, а с кассира уже удержана недостача:
    // удержание само не снимается (ADR-064), владельцу — напоминание, что его можно простить.
    if seq > 1 && diff > 0 {
        let withheld = sqlx::query_scalar!(
            r#"select coalesce(-sum(a.amount_tyiyn), 0)::bigint as "v!" from payroll_accruals a
               where a.doc_type = 'shift' and a.doc_id = $1 and a.kind = 'shortage'
                 and a.reversal_of is null
                 and not exists (select 1 from payroll_accruals r where r.reversal_of = a.id)"#,
            id
        )
        .fetch_one(&mut *tx)
        .await?;
        if withheld > 0 {
            let number = shift.number;
            ops::audit(
                &mut tx,
                &ctx,
                "shift.reclose_surplus",
                "shift",
                Some(id),
                json!({ "number": number, "surplus": diff, "withheld": withheld }),
            )
            .await?;
        }
    }
    let out = load_shift(&mut tx, branch_id, id).await?;
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct ReopenReq {
    op_id: Uuid,
    reason: String,
}

async fn reopen_shift(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<ReopenReq>,
) -> AppResult<Json<ShiftOut>> {
    const KIND: &str = "shift.reopen";
    // Переоткрывает только владелец (ADR-021).
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if req.reason.trim().is_empty() {
        return Err(invalid("укажите причину"));
    }
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let target = sqlx::query!(
        "select account_id, opened_at from shifts where id = $1 and branch_id = $2",
        id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    let account_id = target.account_id;
    lock_accounts(&mut tx, ctx.user.branch_id, &[account_id]).await?;
    // Смен в день несколько (ADR-071): переоткрыть можно только последнюю. После более поздней
    // смены её пересчёт уже учёл эти деньги — старая смена снова открытой стать не может.
    let later = sqlx::query_scalar!(
        r#"select number from shifts
           where branch_id = $1 and account_id = $2 and opened_at > $3 and id <> $4
           order by opened_at limit 1"#,
        ctx.user.branch_id,
        account_id,
        target.opened_at,
        id
    )
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(n) = later {
        return Err(AppError::Conflict(format!(
            "после этой смены уже открывали смену № {n}: переоткрыть можно только последнюю"
        )));
    }
    // Пока открыта другая смена этой кассы, старую не переоткрыть: открытой бывает одна.
    if open_shift_id(&mut tx, ctx.user.branch_id, account_id)
        .await?
        .is_some()
    {
        return Err(AppError::Conflict("сначала закройте открытую смену".into()));
    }
    let close = sqlx::query!(
        r#"select c.id from shift_closes c
           where c.shift_id = $1
             and not exists (select 1 from shift_reopens r where r.close_id = c.id)
           order by c.seq desc limit 1"#,
        id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::Conflict("смена не закрыта".into()))?;
    sqlx::query!(
        r#"insert into shift_reopens (id, shift_id, close_id, reason, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6)"#,
        new_id(),
        id,
        close.id,
        req.reason.trim(),
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "shift",
        Some(id),
        json!({ "reason": req.reason.trim() }),
    )
    .await?;
    let out = load_shift(&mut tx, ctx.user.branch_id, id).await?;
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct HandoverReq {
    op_id: Uuid,
    /// Сколько переложили в сейф; 0 — всё осталось в кассе.
    amount_tyiyn: i64,
    to_account_id: Option<Uuid>,
}

/// После закрытия: перевод выручки в сейф или отметка, что деньги остались в кассе (SPEC-05).
async fn handover_shift(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<HandoverReq>,
) -> AppResult<Json<ShiftOut>> {
    const KIND: &str = "shift.handover";
    let branch_id = ctx.user.branch_id;
    if req.amount_tyiyn < 0 {
        return Err(invalid("сумма не может быть меньше нуля"));
    }
    ops::check_amount(req.amount_tyiyn)?;
    let mut tx = state.pool.begin().await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, req.op_id, KIND).await? {
        return Ok(Json(done));
    }
    let shift = sqlx::query!(
        "select number, account_id from shifts where id = $1 and branch_id = $2",
        id,
        branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    // Куда сдают — выбирает кассир (SPEC-29 §14): по умолчанию куда сдавали в прошлый раз,
    // иначе сейф магазина. Любой действующий счёт, кроме самой кассы смены (ADR-081).
    let safe = match req.to_account_id {
        Some(a) => a,
        None => handover_default(&mut tx, branch_id, shift.account_id)
            .await?
            .ok_or_else(|| invalid("нет сейфа магазина: выберите, куда сдать"))?,
    };
    if safe == shift.account_id {
        return Err(invalid("кассу сдают на другой счёт"));
    }
    let target = sqlx::query!(
        "select name, active from cash_accounts where id = $1 and branch_id = $2",
        safe,
        branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| invalid("касса не найдена"))?;
    if !target.active {
        return Err(invalid("счёт в архиве"));
    }
    let cashier = sqlx::query_scalar!(
        "select e.full_name from shifts s join employees e on e.id = s.cashier_employee_id where s.id = $1",
        id
    )
    .fetch_one(&mut *tx)
    .await?;
    lock_accounts(&mut tx, branch_id, &[shift.account_id, safe]).await?;
    let close = sqlx::query!(
        r#"select c.id, c.expected_tyiyn, c.counted_tyiyn, c.diff_tyiyn,
                  exists (select 1 from shift_handovers h where h.close_id = c.id
                          and (h.transfer_id is null or not exists (select 1 from cash_transfers x where x.reversal_of = h.transfer_id))) as "done!"
           from shift_closes c
           where c.shift_id = $1
             and not exists (select 1 from shift_reopens r where r.close_id = c.id)
           order by c.seq desc limit 1"#,
        id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| AppError::Conflict("смена не закрыта".into()))?;
    if close.done {
        return Err(AppError::Conflict("касса по этой смене уже сдана".into()));
    }
    // В сейф перекладывают пересчитанные наличные: больше, чем лежит в кассе, не переложить —
    // минус кассы (ADR-077) бывает только от проведённых операций, не от сдачи.
    let in_till = sqlx::query_scalar!(
        "select balance_tyiyn from cash_accounts where id = $1",
        shift.account_id
    )
    .fetch_one(&mut *tx)
    .await?;
    if req.amount_tyiyn > in_till.max(0) {
        return Err(invalid(format!(
            "в кассе только {}",
            crate::domain::money::format_som(in_till.max(0))
        )));
    }
    let transfer_id = if req.amount_tyiyn > 0 {
        let comment = format!("сдача кассы, смена № {}", shift.number);
        let (tid, _) = transfer_tx(
            &mut tx,
            &ctx,
            TransferArgs {
                from: shift.account_id,
                to: safe,
                amount: req.amount_tyiyn,
                comment: &comment,
                reversal_of: None,
            },
        )
        .await?;
        Some(tid)
    } else {
        None
    };
    let left = sqlx::query_scalar!(
        "select balance_tyiyn from cash_accounts where id = $1",
        shift.account_id
    )
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query!(
        r#"insert into shift_handovers (id, shift_id, close_id, transfer_id, to_safe_tyiyn,
                                        left_tyiyn, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8)"#,
        new_id(),
        id,
        close.id,
        transfer_id,
        req.amount_tyiyn,
        left,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *tx)
    .await?;
    // Владелец узнаёт итог смены из уведомления, не открывая её (SPEC-13).
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "shift",
        Some(id),
        json!({
            "number": shift.number,
            "expected": close.expected_tyiyn,
            "counted": close.counted_tyiyn,
            "diff": close.diff_tyiyn,
            "to_safe": req.amount_tyiyn,
            "left": left,
            "to_account_id": safe,
            "to_name": target.name,
            "cashier": cashier,
        }),
    )
    .await?;
    let out = load_shift(&mut tx, branch_id, id).await?;
    ops::finish_op(&mut tx, &ctx, req.op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Куда сдать кассу по умолчанию: куда сдавали в прошлый раз (если счёт ещё действует),
/// иначе сейф магазина (SPEC-29 §14).
pub(crate) async fn handover_default(
    conn: &mut PgConnection,
    branch_id: Uuid,
    till: Uuid,
) -> AppResult<Option<Uuid>> {
    Ok(sqlx::query_scalar!(
        r#"select coalesce(
             (select t.to_account_id from shift_handovers h
              join cash_transfers t on t.id = h.transfer_id
              join cash_accounts a on a.id = t.to_account_id
              join shifts s on s.id = h.shift_id
              where s.branch_id = $1 and a.active and a.id <> $2
              order by h.created_at desc limit 1),
             (select id from cash_accounts
              where branch_id = $1 and kind = 'safe' and not owner_only and active
              order by created_at limit 1)) as "id?""#,
        branch_id,
        till
    )
    .fetch_one(&mut *conn)
    .await?)
}

#[derive(Serialize)]
struct HandoverTarget {
    account_id: Option<Uuid>,
}

/// Счёт, который окно «Сдать кассу» предлагает первым.
async fn handover_target(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<HandoverTarget>> {
    let mut conn = state.pool.acquire().await?;
    let till = default_account(&mut conn, user.branch_id).await?;
    Ok(Json(HandoverTarget {
        account_id: handover_default(&mut conn, user.branch_id, till).await?,
    }))
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct MovementReq {
    pub op_id: Uuid,
    pub account_id: Option<Uuid>,
    pub kind: String,
    pub amount_tyiyn: i64,
    pub comment: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct BalanceOut {
    pub account_id: Uuid,
    pub balance_tyiyn: i64,
    /// Проведённое движение (внесение, изъятие или его сторно).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub movement_id: Option<Uuid>,
}

/// Кассы владельца трогает только он (ADR-037).
async fn check_account_access(
    conn: &mut PgConnection,
    ctx: &Ctx,
    account_id: Uuid,
) -> AppResult<()> {
    let owner_only = sqlx::query_scalar!(
        "select owner_only from cash_accounts where id = $1 and branch_id = $2",
        account_id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| invalid("касса не найдена"))?;
    if owner_only && !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

async fn post_movement(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<MovementReq>,
) -> AppResult<Json<BalanceOut>> {
    let mut tx = state.pool.begin().await?;
    let out = post_movement_tx(&mut tx, &ctx, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Внесение или изъятие.
/// Одна транзакция вызывающего: экран и бот владельца (SPEC-24) проводят одной функцией.
pub async fn post_movement_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    req: MovementReq,
) -> AppResult<BalanceOut> {
    const KIND: &str = "cash.movement";
    // Внести, изъять, перевести — право администратора `cash_moves` (ADR-097).
    crate::auth::require_permission(&mut *conn, &ctx.user, "cash_moves").await?;
    if !matches!(req.kind.as_str(), "cash_in" | "cash_out") {
        return Err(invalid("вид движения: внесение или изъятие"));
    }
    if req.amount_tyiyn <= 0 {
        return Err(invalid("сумма больше нуля"));
    }
    crate::ops::check_amount(req.amount_tyiyn)?;
    if req.comment.trim().is_empty() {
        return Err(invalid("укажите, за что"));
    }
    if let Some(done) = ops::begin_op(&mut *conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let account_id = match req.account_id {
        Some(id) => id,
        None => default_account(&mut *conn, ctx.user.branch_id).await?,
    };
    // Из сейфа и со счёта деньги двигает владелец (ADR-037): администратор — только касса смены.
    // Архивный счёт к выбору не предлагается (ADR-076).
    check_pay_account(&mut *conn, &ctx.user, account_id).await?;
    require_open_shift(&mut *conn, ctx.user.branch_id, account_id).await?;
    let amount = if req.kind == "cash_in" {
        req.amount_tyiyn
    } else {
        -req.amount_tyiyn
    };
    let (movement_id, balance) = add_movement_id(
        &mut *conn,
        ctx,
        CashEntry {
            account_id,
            kind: &req.kind,
            amount,
            doc_type: "manual",
            doc_id: None,
            comment: req.comment.trim(),
        },
    )
    .await?;
    ops::audit(
        &mut *conn,
        ctx,
        KIND,
        "cash",
        Some(account_id),
        json!({ "kind": req.kind, "amount_tyiyn": amount, "comment": req.comment.trim() }),
    )
    .await?;
    let out = BalanceOut {
        account_id,
        balance_tyiyn: balance,
        movement_id: Some(movement_id),
    };
    ops::finish_op(&mut *conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

#[derive(Deserialize)]
pub struct ReverseReq {
    pub op_id: Uuid,
    #[serde(default)]
    pub comment: String,
}

/// Администратор отменяет только то, что сделано в открытой смене (или вне смены, пока
/// новую не открыли); владелец — что угодно.
async fn check_reverse_window(
    conn: &mut PgConnection,
    ctx: &Ctx,
    account_id: Uuid,
    shift_id: Option<Uuid>,
) -> AppResult<()> {
    if ctx.user.is_owner() {
        return Ok(());
    }
    // Администратор сторнирует только движения открытой смены (SPEC-05): у счёта без смен
    // (сейф, банк) смены нет ни у движения, ни открытой — это не «та же смена» (ADR-088).
    let open = open_shift_id(conn, ctx.user.branch_id, account_id).await?;
    if shift_id.is_none() || shift_id != open {
        return Err(AppError::Forbidden);
    }
    Ok(())
}

async fn reverse_movement(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<ReverseReq>,
) -> AppResult<Json<BalanceOut>> {
    let mut tx = state.pool.begin().await?;
    let out = reverse_movement_tx(&mut tx, &ctx, id, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Сторно внесения или изъятия — одна транзакция вызывающего: экран и «Исправить» (SPEC-29 §24).
pub async fn reverse_movement_tx(
    tx: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    req: ReverseReq,
) -> AppResult<BalanceOut> {
    const KIND: &str = "cash.reverse";
    crate::auth::require_permission(&mut *tx, &ctx.user, "cash_moves").await?;
    if req.comment.trim().is_empty() {
        return Err(invalid("укажите причину"));
    }
    if let Some(done) = ops::begin_op(&mut *tx, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let m = sqlx::query!(
        r#"select account_id, shift_id, kind, amount_tyiyn, doc_type from cash_movements
           where id = $1 and branch_id = $2"#,
        id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    // Чеки, расходы и выплаты отменяются своим документом, иначе разойдутся с учётом.
    if m.doc_type != "manual" {
        return Err(invalid(
            "это движение отменяется своим документом: чеком, расходом или выплатой",
        ));
    }
    check_account_access(&mut *tx, ctx, m.account_id).await?;
    lock_accounts(&mut *tx, ctx.user.branch_id, &[m.account_id]).await?;
    check_reverse_window(&mut *tx, ctx, m.account_id, m.shift_id).await?;
    let done = sqlx::query_scalar!(
        r#"select exists (select 1 from cash_movements
                          where kind = 'reversal' and doc_type = 'cash_movement' and doc_id = $1) as "e!""#,
        id
    )
    .fetch_one(&mut *tx)
    .await?;
    if done {
        return Err(AppError::Conflict("движение уже отменено".into()));
    }
    let (rid, balance) = add_movement_id(
        &mut *tx,
        ctx,
        CashEntry {
            account_id: m.account_id,
            kind: "reversal",
            amount: -m.amount_tyiyn,
            doc_type: "cash_movement",
            doc_id: Some(id),
            comment: req.comment.trim(),
        },
    )
    .await?;
    ops::audit(
        &mut *tx,
        ctx,
        KIND,
        "cash",
        Some(id),
        json!({ "kind": m.kind, "amount_tyiyn": -m.amount_tyiyn, "comment": req.comment.trim() }),
    )
    .await?;
    let out = BalanceOut {
        account_id: m.account_id,
        balance_tyiyn: balance,
        movement_id: Some(rid),
    };
    ops::finish_op(&mut *tx, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct TransferReq {
    pub op_id: Uuid,
    pub from_account_id: Uuid,
    pub to_account_id: Uuid,
    pub amount_tyiyn: i64,
    #[serde(default)]
    pub comment: String,
}

struct TransferArgs<'a> {
    from: Uuid,
    to: Uuid,
    amount: i64,
    comment: &'a str,
    reversal_of: Option<Uuid>,
}

/// Перемещение: запись и два движения с общим `doc_id`. Кассы должны быть заблокированы.
async fn transfer_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    a: TransferArgs<'_>,
) -> AppResult<(Uuid, i64)> {
    let active = sqlx::query_scalar!(
        "select active from cash_accounts where id = $1 and branch_id = $2",
        a.to,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or_else(|| invalid("касса не найдена"))?;
    if !active {
        return Err(invalid("касса-получатель отключена"));
    }
    let number = ops::next_counter(conn, ctx.user.branch_id, "cash_transfer").await?;
    let id = new_id();
    sqlx::query!(
        r#"insert into cash_transfers (id, branch_id, number, from_account_id, to_account_id,
                                       amount_tyiyn, comment, reversal_of, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"#,
        id,
        ctx.user.branch_id,
        number,
        a.from,
        a.to,
        a.amount,
        a.comment,
        a.reversal_of,
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *conn)
    .await?;
    let (out_kind, in_kind) = if a.reversal_of.is_some() {
        ("reversal", "reversal")
    } else {
        ("transfer_out", "transfer_in")
    };
    add_movement(
        conn,
        ctx,
        CashEntry {
            account_id: a.from,
            kind: out_kind,
            amount: -a.amount,
            doc_type: "transfer",
            doc_id: Some(id),
            comment: a.comment,
        },
    )
    .await?;
    add_movement(
        conn,
        ctx,
        CashEntry {
            account_id: a.to,
            kind: in_kind,
            amount: a.amount,
            doc_type: "transfer",
            doc_id: Some(id),
            comment: a.comment,
        },
    )
    .await?;
    Ok((id, number))
}

async fn post_transfer(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<TransferReq>,
) -> AppResult<Json<Value>> {
    let mut tx = state.pool.begin().await?;
    let out = post_transfer_tx(&mut tx, &ctx, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Перемещение между кассами.
/// Одна транзакция вызывающего: экран и бот владельца (SPEC-24) проводят одной функцией.
pub async fn post_transfer_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    req: TransferReq,
) -> AppResult<Value> {
    const KIND: &str = "cash.transfer";
    crate::auth::require_permission(&mut *conn, &ctx.user, "cash_moves").await?;
    if req.from_account_id == req.to_account_id {
        return Err(invalid("кассы должны быть разные"));
    }
    if req.amount_tyiyn <= 0 {
        return Err(invalid("сумма больше нуля"));
    }
    crate::ops::check_amount(req.amount_tyiyn)?;
    if let Some(done) = ops::begin_op(&mut *conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    // Администратор переводит только из кассы смены: из сейфов и со счёта деньги двигает
    // владелец (SPEC-05, права; ADR-037). Из архивного счёта не переводят (ADR-076).
    check_pay_account(&mut *conn, &ctx.user, req.from_account_id).await?;
    lock_accounts(
        &mut *conn,
        ctx.user.branch_id,
        &[req.from_account_id, req.to_account_id],
    )
    .await?;
    let (id, number) = transfer_tx(
        &mut *conn,
        ctx,
        TransferArgs {
            from: req.from_account_id,
            to: req.to_account_id,
            amount: req.amount_tyiyn,
            comment: req.comment.trim(),
            reversal_of: None,
        },
    )
    .await?;
    ops::audit(
        &mut *conn,
        ctx,
        KIND,
        "cash",
        Some(id),
        json!({ "amount_tyiyn": req.amount_tyiyn, "number": number }),
    )
    .await?;
    let out = json!({ "id": id, "number": number });
    ops::finish_op(&mut *conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

async fn reverse_transfer(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<ReverseReq>,
) -> AppResult<Json<Value>> {
    let mut tx = state.pool.begin().await?;
    let out = reverse_transfer_tx(&mut tx, &ctx, id, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Сторно перемещения — одна транзакция вызывающего: экран и «Исправить» (SPEC-29 §24).
pub async fn reverse_transfer_tx(
    tx: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    req: ReverseReq,
) -> AppResult<Value> {
    const KIND: &str = "cash.transfer_reverse";
    crate::auth::require_permission(&mut *tx, &ctx.user, "cash_moves").await?;
    if req.comment.trim().is_empty() {
        return Err(invalid("укажите причину"));
    }
    if let Some(done) = ops::begin_op(&mut *tx, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    let t = sqlx::query!(
        r#"select number, from_account_id, to_account_id, amount_tyiyn, reversal_of
           from cash_transfers where id = $1 and branch_id = $2"#,
        id,
        ctx.user.branch_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(AppError::NotFound)?;
    if t.reversal_of.is_some() {
        return Err(invalid("это уже сторно перемещения"));
    }
    // Сторно забирает деньги из получателя: из кассы владельца — только он сам.
    check_account_access(&mut *tx, ctx, t.to_account_id).await?;
    lock_accounts(
        &mut *tx,
        ctx.user.branch_id,
        &[t.from_account_id, t.to_account_id],
    )
    .await?;
    let out_shift = sqlx::query_scalar!(
        r#"select shift_id from cash_movements
           where doc_type = 'transfer' and doc_id = $1 and account_id = $2"#,
        id,
        t.from_account_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .flatten();
    check_reverse_window(&mut *tx, ctx, t.from_account_id, out_shift).await?;
    // Сдача кассы уже вошла в итог смены, который видел владелец: отменяет её только он.
    let handover = sqlx::query_scalar!(
        r#"select exists (select 1 from shift_handovers where transfer_id = $1) as "e!""#,
        id
    )
    .fetch_one(&mut *tx)
    .await?;
    if handover && !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let done = sqlx::query_scalar!(
        r#"select exists (select 1 from cash_transfers where reversal_of = $1) as "e!""#,
        id
    )
    .fetch_one(&mut *tx)
    .await?;
    if done {
        return Err(AppError::Conflict("перемещение уже отменено".into()));
    }
    let comment = format!("сторно перемещения № {}: {}", t.number, req.comment.trim());
    let (rid, number) = transfer_tx(
        &mut *tx,
        ctx,
        TransferArgs {
            from: t.to_account_id,
            to: t.from_account_id,
            amount: t.amount_tyiyn,
            comment: &comment,
            reversal_of: Some(id),
        },
    )
    .await?;
    ops::audit(
        &mut *tx,
        ctx,
        KIND,
        "cash",
        Some(rid),
        json!({
            "amount_tyiyn": t.amount_tyiyn,
            "number": t.number,
            "comment": req.comment.trim(),
        }),
    )
    .await?;
    let out = json!({ "id": rid, "number": number });
    ops::finish_op(&mut *tx, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

#[derive(Serialize)]
pub(crate) struct MovementOut {
    pub(crate) id: Uuid,
    pub(crate) kind: String,
    pub(crate) amount_tyiyn: i64,
    pub(crate) comment: String,
    pub(crate) doc_type: String,
    pub(crate) doc_id: Option<Uuid>,
    pub(crate) user_name: String,
    pub(crate) created_at: DateTime<Utc>,
    /// Можно отменить сторно: внесение, изъятие или перемещение, ещё не отменённые.
    pub(crate) reversible: bool,
}

async fn account_movements(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Query(period): Query<PeriodQuery>,
    Query(page): Query<crate::api::paging::PageQuery>,
) -> AppResult<crate::api::paging::Paged<MovementOut>> {
    let mut conn = state.pool.acquire().await?;
    // История счёта за период (SPEC-28 §5); без дат — все движения, как раньше.
    let period = match (period.from, period.to) {
        (None, None) => None,
        (from, to) => {
            Some(crate::api::reports::resolve_period(&mut conn, from.or(to), to.or(from)).await?)
        }
    };
    let rows =
        movements_between(&mut conn, &user, id, period, crate::api::paging::MAX_ROWS).await?;
    let (items, total) = page.slice(rows, 200);
    Ok(crate::api::paging::paged(items, total))
}

/// Последние 200 движений кассы — экран и бот владельца (SPEC-24).
pub(crate) async fn movements_data(
    conn: &mut PgConnection,
    user: &CurrentUser,
    id: Uuid,
) -> AppResult<Vec<MovementOut>> {
    movements_between(conn, user, id, None, 200).await
}

/// Движения кассы, новые сверху: все или за дни `period` по Бишкеку (экран «Счёт», SPEC-27 §6).
pub(crate) async fn movements_between(
    conn: &mut PgConnection,
    user: &CurrentUser,
    id: Uuid,
    period: Option<(NaiveDate, NaiveDate)>,
    limit: i64,
) -> AppResult<Vec<MovementOut>> {
    // Движения сейфа владельца администратору не отдаём, как и его остаток (ADR-037).
    let owner_only = sqlx::query_scalar!(
        "select owner_only from cash_accounts where id = $1 and branch_id = $2",
        id,
        user.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    if owner_only && !user.is_owner() {
        return Err(AppError::NotFound);
    }
    let rows = sqlx::query_as!(
        MovementOut,
        r#"select m.id, m.kind, m.amount_tyiyn,
                  -- Личные расходы владельца администратору — только суммой (SPEC-06, ADR-088):
                  -- остаток кассы должен сходиться, а на что потрачено — не его дело.
                  case when not $6 and m.doc_type = 'expense' and exists (
                         select 1 from expenses e join expense_articles a on a.id = e.article_id
                         where e.id = m.doc_id and a.owner_only)
                       then 'расход владельца' else m.comment end as "comment!",
                  m.doc_type, m.doc_id,
                  u.full_name as user_name, m.created_at,
                  case
                    when m.doc_type = 'manual' then not exists (
                      select 1 from cash_movements r
                      where r.kind = 'reversal' and r.doc_type = 'cash_movement' and r.doc_id = m.id)
                    when m.doc_type = 'transfer' and m.kind in ('transfer_in', 'transfer_out') then not exists (
                      select 1 from cash_transfers t where t.reversal_of = m.doc_id)
                    else false
                  end as "reversible!"
           from cash_movements m join users u on u.id = m.user_id
           where m.account_id = $1 and m.branch_id = $2
             and ($3::date is null or m.created_at >= ($3::date)::timestamp at time zone 'Asia/Bishkek')
             and ($4::date is null or m.created_at < ($4::date + 1)::timestamp at time zone 'Asia/Bishkek')
           order by m.created_at desc, m.id desc limit $5"#,
        id,
        user.branch_id,
        period.map(|p| p.0),
        period.map(|p| p.1),
        limit,
        user.is_owner()
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows)
}

/// Итоги счёта за период для истории в «Обзоре счетов» (SPEC-28 §5): на начало, по видам
/// движений, на конец — те же, что «Куда сколько поступило» по кассам.
async fn account_summary(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
    Query(q): Query<PeriodQuery>,
) -> AppResult<Json<crate::api::income::AccountFlow>> {
    let mut conn = state.pool.acquire().await?;
    let (from, to) = crate::api::reports::resolve_period(&mut conn, q.from, q.to).await?;
    let flows = crate::api::income::flows_of(&mut conn, &user, from, to, Some(id)).await?;
    flows.into_iter().next().map(Json).ok_or(AppError::NotFound)
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct AccountCreateReq {
    pub op_id: Uuid,
    pub name: String,
    /// `bank` — безналичный, `safe` или `other` — наличный. Касса смены одна, новую не заводят.
    pub kind: String,
    /// Можно уйти ниже нуля: по умолчанию — да (ADR-077).
    #[serde(default = "yes")]
    pub allow_negative: bool,
    #[serde(default)]
    pub owner_only: bool,
    /// Ставка комиссии банка с сегодняшнего дня, сотые доли процента (только `bank`; нет — 0 %).
    #[serde(default)]
    pub fee_rate_bp: Option<i32>,
    /// Сразу сделать счётом по умолчанию для карты (только `bank`): прежний перестаёт им быть.
    #[serde(default)]
    pub default_card: Option<bool>,
    /// Сразу сделать счётом по умолчанию для перевода (только `bank`): прежний перестаёт им быть.
    #[serde(default)]
    pub default_transfer: Option<bool>,
    /// Сразу сделать счётом по умолчанию для «По счёту» (только `bank`, ADR-095).
    #[serde(default)]
    pub default_invoice: Option<bool>,
}

fn account_name(name: &str) -> AppResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid("укажите название счёта"));
    }
    if name.chars().count() > 60 {
        return Err(invalid("название счёта — не длиннее 60 знаков"));
    }
    Ok(name.to_string())
}

fn unique_name(e: sqlx::Error) -> AppError {
    if crate::error::is_unique_violation(&e) {
        AppError::Conflict("счёт с таким названием уже есть".into())
    } else {
        AppError::from(e)
    }
}

async fn create_account(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<AccountCreateReq>,
) -> AppResult<Json<AccountOut>> {
    let mut tx = state.pool.begin().await?;
    let out = create_account_tx(&mut tx, &ctx, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Новый счёт заводит только владелец (SPEC-28 §5, ADR-076). Остаток — ноль: деньги приходят
/// внесением или перемещением, как на любую кассу (остаток = сумма движений).
pub async fn create_account_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    req: AccountCreateReq,
) -> AppResult<AccountOut> {
    const KIND: &str = "cash.account_create";
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let name = account_name(&req.name)?;
    if !matches!(req.kind.as_str(), "bank" | "safe" | "other") {
        return Err(invalid(
            "тип счёта: безналичный (bank) или наличный (safe, other); касса смены одна",
        ));
    }
    if let Some(done) = ops::begin_op(&mut *conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    ensure_accounts(&mut *conn, ctx.user.branch_id).await?;
    if req.kind != "bank" && req.fee_rate_bp.is_some_and(|r| r != 0) {
        return Err(invalid("комиссия банка — только у безналичного счёта"));
    }
    let want_card = req.default_card == Some(true);
    let want_transfer = req.default_transfer == Some(true);
    let for_invoice = req.default_invoice == Some(true);
    if req.kind != "bank" && (want_card || want_transfer || for_invoice) {
        return Err(invalid(
            "по умолчанию для способа оплаты — только безналичный счёт",
        ));
    }
    // Первый безналичный счёт филиала сразу становится счётом по умолчанию для карты и перевода;
    // отметки из запроса ставятся тут же, в той же транзакции (SPEC-29 §25).
    let (for_card, for_transfer) = if req.kind == "bank" {
        let d = sqlx::query!(
            r#"select not exists (select 1 from cash_accounts
                                  where branch_id = $1 and default_card) as "card!",
                      not exists (select 1 from cash_accounts
                                  where branch_id = $1 and default_transfer) as "transfer!""#,
            ctx.user.branch_id
        )
        .fetch_one(&mut *conn)
        .await?;
        (d.card || want_card, d.transfer || want_transfer)
    } else {
        (false, false)
    };
    if (for_card || for_transfer || for_invoice) && req.owner_only {
        return Err(invalid(
            "счёт по умолчанию для способа оплаты видит администратор: «только владелец» для него нельзя",
        ));
    }
    if for_invoice {
        sqlx::query!(
            "update cash_accounts set default_invoice = false where branch_id = $1 and default_invoice",
            ctx.user.branch_id
        )
        .execute(&mut *conn)
        .await?;
    }
    if for_card {
        sqlx::query!(
            "update cash_accounts set default_card = false where branch_id = $1 and default_card",
            ctx.user.branch_id
        )
        .execute(&mut *conn)
        .await?;
    }
    if for_transfer {
        sqlx::query!(
            "update cash_accounts set default_transfer = false where branch_id = $1 and default_transfer",
            ctx.user.branch_id
        )
        .execute(&mut *conn)
        .await?;
    }
    let id = new_id();
    let mut out = sqlx::query_as!(
        AccountOut,
        r#"insert into cash_accounts (id, branch_id, name, kind, owner_only, is_default, allow_negative,
                                      default_card, default_transfer, default_invoice)
           values ($1, $2, $3, $4, $5, false, $6, $7, $8, $9)
           returning id, name, kind, owner_only, is_default, default_card, default_transfer, default_invoice, allow_negative,
                  active, balance_tyiyn,
                  account_fee_on(id, (now() at time zone 'Asia/Bishkek')::date) as "fee_rate_bp!""#,
        id,
        ctx.user.branch_id,
        name,
        req.kind,
        req.owner_only,
        req.allow_negative,
        for_card,
        for_transfer,
        for_invoice
    )
    .fetch_one(&mut *conn)
    .await
    .map_err(unique_name)?;
    if let Some(rate) = req.fee_rate_bp.filter(|r| *r != 0) {
        let today = crate::api::fees::today(&mut *conn).await?;
        crate::api::fees::set_rate(&mut *conn, ctx, id, &out.name, rate, today).await?;
        out.fee_rate_bp = rate;
    }
    ops::audit(
        &mut *conn,
        ctx,
        KIND,
        "cash_account",
        Some(id),
        json!({
            "name": out.name,
            "kind": out.kind,
            "owner_only": out.owner_only,
            "allow_negative": out.allow_negative,
            "default_card": out.default_card,
            "default_transfer": out.default_transfer,
            "default_invoice": out.default_invoice,
            "fee_rate_bp": out.fee_rate_bp,
        }),
    )
    .await?;
    ops::finish_op(&mut *conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct AccountPatch {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub allow_negative: Option<bool>,
    #[serde(default)]
    pub owner_only: Option<bool>,
    /// `false` — в архив (только с нулевым остатком), `true` — вернуть из архива.
    #[serde(default)]
    pub active: Option<bool>,
    /// Сделать счётом по умолчанию для карты (только `true`: прежний перестаёт им быть).
    #[serde(default)]
    pub default_card: Option<bool>,
    /// Сделать счётом по умолчанию для перевода (только `true`: прежний перестаёт им быть).
    #[serde(default)]
    pub default_transfer: Option<bool>,
    /// «По счёту» (ADR-095): `true` — сделать этим счётом, `false` — снять отметку (тогда
    /// «По счёту» ложится на счёт карты).
    #[serde(default)]
    pub default_invoice: Option<bool>,
    /// Новая ставка комиссии банка, сотые доли процента (только `bank`).
    #[serde(default)]
    pub fee_rate_bp: Option<i32>,
    /// С какого дня действует новая ставка; без даты — с сегодняшнего.
    #[serde(default)]
    pub fee_from: Option<NaiveDate>,
}

async fn update_account(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<AccountPatch>,
) -> AppResult<Json<AccountOut>> {
    let mut tx = state.pool.begin().await?;
    let out = update_account_tx(&mut tx, &ctx, id, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Отметка «по умолчанию» для способа оплаты: `Some(true)` — сделать этим счётом, снять
/// напрямую нельзя — только выбором другого (ADR-077).
fn default_mark(
    want: Option<bool>,
    was: bool,
    kind: &str,
    active: bool,
    owner_only: bool,
    what: &str,
) -> AppResult<bool> {
    match want {
        Some(true) if !was => {
            if kind != "bank" {
                return Err(invalid(format!(
                    "по умолчанию для {what} — только безналичный счёт"
                )));
            }
            if !active || owner_only {
                return Err(invalid(format!(
                    "счёт по умолчанию для {what} — действующий и не «только владелец»"
                )));
            }
            Ok(true)
        }
        Some(false) if was => Err(invalid(format!(
            "счёт по умолчанию для {what} меняется выбором другого счёта"
        ))),
        _ => Ok(false),
    }
}

/// Правка счёта владельцем (SPEC-28 §5, ADR-076, ADR-077): название, «отрицательный баланс
/// разрешён», «только владелец», архив, счёт по умолчанию для карты и для перевода, ставка
/// комиссии банка. Тип не меняется: у счёта уже есть история. Удаления нет — только архив.
/// В журнал — «было / стало».
pub async fn update_account_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    req: AccountPatch,
) -> AppResult<AccountOut> {
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let branch_id = ctx.user.branch_id;
    // Под блокировкой: остаток для архива и отметки по умолчанию не меняются, пока правим.
    let before = sqlx::query_as!(
        AccountOut,
        r#"select id, name, kind, owner_only, is_default, default_card, default_transfer, default_invoice, allow_negative,
                  active, balance_tyiyn,
                  account_fee_on(id, (now() at time zone 'Asia/Bishkek')::date) as "fee_rate_bp!"
           from cash_accounts where id = $1 and branch_id = $2 for update"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    let name = match &req.name {
        Some(n) => account_name(n)?,
        None => before.name.clone(),
    };
    let allow_negative = req.allow_negative.unwrap_or(before.allow_negative);
    let owner_only = req.owner_only.unwrap_or(before.owner_only);
    let active = req.active.unwrap_or(before.active);
    let till = before.kind == "register" && before.is_default;
    let make_card = default_mark(
        req.default_card,
        before.default_card,
        &before.kind,
        active,
        owner_only,
        "карты",
    )?;
    let make_transfer = default_mark(
        req.default_transfer,
        before.default_transfer,
        &before.kind,
        active,
        owner_only,
        "перевода",
    )?;
    // «По счёту»: отметку можно и снять — тогда деньги ложатся на счёт карты (ADR-095).
    let invoice_mark = match req.default_invoice {
        Some(true) if !before.default_invoice => Some(default_mark(
            Some(true),
            false,
            &before.kind,
            active,
            owner_only,
            "«По счёту»",
        )?),
        Some(false) if before.default_invoice => Some(false),
        _ => None,
    };
    let invoice_after = invoice_mark.unwrap_or(before.default_invoice);
    let pay_default = before.default_card || before.default_transfer || invoice_after;
    if (till || pay_default) && owner_only {
        return Err(invalid(
            "с кассой смены и счетами по умолчанию для способов оплаты работает администратор: «только владелец» для них нельзя",
        ));
    }
    if (till || pay_default) && !active {
        return Err(invalid(if till {
            "кассу смены в архив не убрать"
        } else {
            "счёт по умолчанию для способа оплаты в архив не убрать: сначала отметьте другой"
        }));
    }
    if !active && before.active && before.balance_tyiyn != 0 {
        return Err(invalid(format!(
            "на счёте «{}» остаток {}: переведите деньги на другой счёт, потом в архив",
            before.name,
            crate::domain::money::format_som(before.balance_tyiyn)
        )));
    }
    if req.fee_rate_bp.is_some() && before.kind != "bank" {
        return Err(invalid("комиссия банка — только у безналичного счёта"));
    }
    if make_card {
        sqlx::query!(
            "update cash_accounts set default_card = false where branch_id = $1 and default_card",
            branch_id
        )
        .execute(&mut *conn)
        .await?;
    }
    if make_transfer {
        sqlx::query!(
            "update cash_accounts set default_transfer = false where branch_id = $1 and default_transfer",
            branch_id
        )
        .execute(&mut *conn)
        .await?;
    }
    if invoice_mark == Some(true) {
        sqlx::query!(
            "update cash_accounts set default_invoice = false where branch_id = $1 and default_invoice",
            branch_id
        )
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query!(
        r#"update cash_accounts
           set name = $3, allow_negative = $4, owner_only = $5, active = $6,
               default_card = default_card or $7, default_transfer = default_transfer or $8,
               default_invoice = $9
           where id = $1 and branch_id = $2"#,
        id,
        branch_id,
        name,
        allow_negative,
        owner_only,
        active,
        make_card,
        make_transfer,
        invoice_after
    )
    .execute(&mut *conn)
    .await
    .map_err(unique_name)?;
    // Ставка комиссии — своя история с датой начала (SPEC-08): прошлые дни сохраняют свою.
    if let Some(rate) = req.fee_rate_bp {
        let from = match req.fee_from {
            Some(d) => d,
            None => crate::api::fees::today(&mut *conn).await?,
        };
        crate::api::fees::set_rate(&mut *conn, ctx, id, &name, rate, from).await?;
    }
    let out = sqlx::query_as!(
        AccountOut,
        r#"select id, name, kind, owner_only, is_default, default_card, default_transfer, default_invoice, allow_negative,
                  active, balance_tyiyn,
                  account_fee_on(id, (now() at time zone 'Asia/Bishkek')::date) as "fee_rate_bp!"
           from cash_accounts where id = $1"#,
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    let view = |a: &AccountOut| {
        json!({
            "name": a.name,
            "allow_negative": a.allow_negative,
            "owner_only": a.owner_only,
            "active": a.active,
            "default_card": a.default_card,
            "default_transfer": a.default_transfer,
            "default_invoice": a.default_invoice,
        })
    };
    let changes = ops::changes(&view(&before), &view(&out));
    if changes.as_object().is_some_and(|c| !c.is_empty()) {
        ops::audit(
            &mut *conn,
            ctx,
            "cash.account_update",
            "cash_account",
            Some(id),
            json!({ "name": out.name, "before": view(&before), "after": view(&out), "changes": changes }),
        )
        .await?;
    }
    Ok(out)
}
