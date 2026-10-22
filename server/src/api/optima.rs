//! Optima Business (SPEC-31, ADR-106): подключение к API банка по ключу `OPTIMA_API_KEY`,
//! настройка «Optima Business» владельцем и фоновая работа — опрос QR каждые 2 с, «QR оплачен,
//! чек не проведён», ежедневная сверка с выпиской в 06:00.
//!
//! Суммы банка — дробь с двумя знаками; в тыйыны они переводятся разбором текста числа из JSON
//! (`RawValue`), без дробных чисел (инвариант 1). Ключ не печатается и не пишется в журналы.

use std::time::Duration;

use axum::extract::{Query, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use serde_json::{Value, json};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use crate::auth::{Ctx, CurrentUser};
use crate::config::SecretKey;
use crate::error::{AppError, AppResult, invalid};
use crate::state::AppState;

pub const DEFAULT_URL: &str = "https://api.optimabusiness.kg";

/// Запрос QR и его статуса — короткий: касса ждёт.
const QUICK: Duration = Duration::from_secs(10);
/// Выписка за 14 дней может собираться дольше.
const SLOW: Duration = Duration::from_secs(90);

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/optima/status", routing::get(status))
        .route(
            "/settings/optima",
            routing::get(get_settings).put(put_settings),
        )
        .route("/settings/optima/sale-points", routing::get(sale_points))
        .route("/settings/optima/check", routing::post(check))
}

// ---------- Суммы и время банка ----------

/// Сумма банка из текста числа JSON («990.00», «38», «1500.5», «"12.30"») — в тыйыны, без
/// дробных чисел. Больше двух знаков после точки (кроме нулей), экспонента, пусто — `None`.
pub fn parse_money(raw: &str) -> Option<i64> {
    let s = raw.trim();
    let s = s
        .strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .unwrap_or(s)
        .trim();
    let (neg, s) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let (whole, frac) = match s.split_once('.') {
        Some((w, f)) => (w, f),
        None => (s, ""),
    };
    if whole.is_empty() || !whole.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if !frac.bytes().all(|b| b.is_ascii_digit()) || (s.contains('.') && frac.is_empty()) {
        return None;
    }
    // Третий и дальше знаки — только нули: «990.000» ещё сумма, «0.005» — уже нет.
    if frac.len() > 2 && !frac[2..].bytes().all(|b| b == b'0') {
        return None;
    }
    let cents: i64 = match frac.len() {
        0 => 0,
        1 => i64::from(frac.as_bytes()[0] - b'0') * 10,
        _ => frac[..2].parse().ok()?,
    };
    let v = whole
        .parse::<i64>()
        .ok()?
        .checked_mul(100)?
        .checked_add(cents)?;
    Some(if neg { -v } else { v })
}

/// Сумма из поля ответа: число или строка; нет поля или `null` — `None`.
fn money_of(v: Option<&RawValue>) -> Option<i64> {
    v.map(RawValue::get)
        .filter(|s| s.trim() != "null")
        .and_then(parse_money)
}

/// Тыйыны — числом JSON с двумя знаками («1500.50») для запроса в банк.
pub fn money_json(t: i64) -> AppResult<Box<RawValue>> {
    let sign = if t < 0 { "-" } else { "" };
    let a = t.unsigned_abs();
    RawValue::from_string(format!("{sign}{}.{:02}", a / 100, a % 100))
        .map_err(|e| AppError::Internal(e.to_string()))
}

/// Бишкек — UTC+6 круглый год.
pub fn bishkek() -> FixedOffset {
    FixedOffset::east_opt(6 * 3600).unwrap_or_else(|| chrono::Offset::fix(&Utc))
}

/// Время банка: с зоной («…Z», «+06:00») — как есть, без зоны — время Бишкека.
pub fn parse_bank_time(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Some(t.with_timezone(&Utc));
    }
    let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f"))
        .ok()?;
    bishkek()
        .from_local_datetime(&naive)
        .single()
        .map(|t| t.with_timezone(&Utc))
}

// ---------- Подключение к банку ----------

/// Что пошло не так при запросе в банк.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BankError {
    /// 401/403: ключ не подходит или не связан с компанией.
    Key(String),
    /// 404 (1003): не найдено — у статуса QR это «ещё не оплачен».
    NotFound,
    /// Банк отклонил запрос (400, 1004 и т. п.) — с его текстом.
    Rejected(String),
    /// Нет связи, таймаут, 5xx, непонятный ответ.
    Unavailable(String),
}

impl BankError {
    /// Текст для кассы и владельца.
    pub fn message(&self) -> String {
        match self {
            Self::Key(_) => "Ключ Optima Business не подходит — владельцу: проверьте ключ".into(),
            Self::NotFound => "банк не нашёл операцию".into(),
            Self::Rejected(m) => format!("Банк отклонил: {m}"),
            Self::Unavailable(_) => "Банк недоступен — отметьте оплату вручную".into(),
        }
    }
}

impl From<BankError> for AppError {
    fn from(e: BankError) -> Self {
        AppError::Bank(e.message())
    }
}

/// Подключение к API Optima Business.
#[derive(Clone)]
pub struct Optima {
    http: reqwest::Client,
    key: SecretKey,
    url: String,
}

impl std::fmt::Debug for Optima {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Optima")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

/// Торговая точка и её кассы из банка.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SalePoint {
    pub account: String,
    pub name: String,
    pub address: String,
    pub code: i64,
    pub cashes: Vec<CashDesk>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct CashDesk {
    pub name: String,
    pub code: i64,
}

/// Новый QR от банка.
#[derive(Debug, Clone)]
pub struct NewQr {
    pub transaction_id: String,
    pub qr_base64: String,
    pub qr_url: String,
}

/// Состояние QR в банке.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrState {
    /// `PROCESSED`, `NOT_EXIST`, `ERROR` или другое, что пришлёт банк.
    pub status: String,
    pub sum: Option<i64>,
    pub operation_id: Option<String>,
    pub processed_at: Option<DateTime<Utc>>,
}

/// Операция выписки по POS и QR.
#[derive(Debug, Clone)]
pub struct StatementOp {
    pub key: String,
    pub kind: &'static str,
    pub processed_at: DateTime<Utc>,
    pub amount: i64,
    pub fee: i64,
    pub transfer: i64,
    pub auth_code: String,
    pub rrn: String,
    pub terminal: String,
    pub requisite: String,
    /// Операция как прислал банк — текстом JSON, числа не переводятся.
    pub raw: String,
}

/// Параметры QR на сумму.
pub struct QrRequest<'a> {
    pub legal_party_id: i64,
    pub sale_point_code: i64,
    pub cash_code: i64,
    pub amount: i64,
    pub until: DateTime<Utc>,
    pub note: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Requisite {
    sale_point_code: i64,
    cash_code: i64,
    legal_party_id: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GenerateBody<'a> {
    requisite: Requisite,
    sum: Box<RawValue>,
    allow_edit_sum: bool,
    note: &'a str,
    qr_generate_type: &'a str,
    transaction_count: i64,
    until_date_time: String,
    qr_type: &'a str,
    qr_size: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GenerateReply {
    #[serde(default)]
    qr_base64: String,
    #[serde(default)]
    qr_url: String,
    #[serde(default)]
    transaction_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatusReply {
    #[serde(default)]
    status: String,
    #[serde(default)]
    sum: Option<Box<RawValue>>,
    #[serde(default)]
    operation_id: Option<String>,
    #[serde(default)]
    transaction_processed_date_time: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatementReply {
    #[serde(default)]
    pos_operations: Vec<Box<RawValue>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PosOp {
    #[serde(default)]
    operation_processed_date_time: Option<String>,
    #[serde(default)]
    pos_terminal_code: Option<String>,
    #[serde(default)]
    auth_code: Option<String>,
    #[serde(default)]
    requisite: Option<String>,
    #[serde(default)]
    operation_sum: Option<Box<RawValue>>,
    #[serde(default)]
    operation_fee_sum: Option<Box<RawValue>>,
    #[serde(default)]
    operation_transfer_sum: Option<Box<RawValue>>,
    #[serde(default)]
    rrn: Option<String>,
    #[serde(default)]
    stan: Option<Value>,
    #[serde(default)]
    qr_type: Option<String>,
    #[serde(default)]
    operation_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PointsReply {
    #[serde(default)]
    account: Option<Value>,
    #[serde(default)]
    sale_point_info_dto_list: Vec<PointReply>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PointReply {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    address: Option<String>,
    #[serde(default)]
    code: Option<Value>,
    #[serde(default)]
    cash_dto_list: Vec<CashReply>,
}

#[derive(Deserialize)]
struct CashReply {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    code: Option<Value>,
}

/// Код точки или кассы: число или строка с числом.
fn code_of(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn text_of(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// Ключ операции выписки: RRN, а без него — время, сумма, код авторизации и терминал.
fn op_key(op: &PosOp, time: &str, amount: i64) -> String {
    if let Some(r) = op.rrn.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
        return format!("rrn:{r}");
    }
    let stan = text_of(op.stan.as_ref());
    format!(
        "op:{time}|{amount}|{}|{}|{}",
        op.auth_code.as_deref().unwrap_or(""),
        op.pos_terminal_code.as_deref().unwrap_or(""),
        stan
    )
}

/// Разбор операции выписки; кривую строку пропускаем (в журнал — без данных).
fn statement_op(raw: &RawValue) -> Option<StatementOp> {
    let op: PosOp = serde_json::from_str(raw.get()).ok()?;
    let time = op.operation_processed_date_time.clone()?;
    let processed_at = parse_bank_time(&time)?;
    let amount = money_of(op.operation_sum.as_deref())?;
    let fee = money_of(op.operation_fee_sum.as_deref()).unwrap_or(0);
    let transfer =
        money_of(op.operation_transfer_sum.as_deref()).unwrap_or(amount.saturating_sub(fee));
    // Вид операции в выписке по POS явно не назван: у QR есть вид QR или STAN, нет терминала.
    let qr = op.qr_type.as_deref().is_some_and(|t| !t.trim().is_empty())
        || op
            .operation_name
            .as_deref()
            .is_some_and(|n| n.to_uppercase().contains("QR"))
        || (op.stan.is_some()
            && op
                .pos_terminal_code
                .as_deref()
                .is_none_or(|t| t.trim().is_empty()));
    let rrn = op
        .rrn
        .clone()
        .filter(|r| !r.trim().is_empty())
        .unwrap_or_else(|| text_of(op.stan.as_ref()));
    Some(StatementOp {
        key: op_key(&op, &time, amount),
        kind: if qr { "qr" } else { "card" },
        processed_at,
        amount,
        fee,
        transfer,
        auth_code: op.auth_code.unwrap_or_default(),
        rrn,
        terminal: op.pos_terminal_code.unwrap_or_default(),
        requisite: op.requisite.unwrap_or_default(),
        raw: raw.get().to_string(),
    })
}

/// Текст ошибки банка `{error, code}` (код бывает строкой), не длиннее 200 знаков.
fn error_text(raw: &[u8]) -> String {
    let v: Value = serde_json::from_slice(raw).unwrap_or(Value::Null);
    let msg = v
        .get("error")
        .and_then(Value::as_str)
        .or_else(|| v.get("message").and_then(Value::as_str))
        .unwrap_or("")
        .trim()
        .to_string();
    let code = text_of(v.get("code"));
    let text = match (msg.is_empty(), code.is_empty()) {
        (true, true) => String::from("без пояснения"),
        (false, true) => msg,
        (true, false) => format!("код {code}"),
        (false, false) => format!("{msg} (код {code})"),
    };
    crate::api::bot_drafts::one_line(&text)
        .chars()
        .take(200)
        .collect()
}

impl Optima {
    pub fn new(key: SecretKey, url: Option<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            key,
            url: url
                .unwrap_or_else(|| DEFAULT_URL.into())
                .trim_end_matches('/')
                .to_string(),
        }
    }

    /// Запрос в банк; ответ — тело при 2xx, иначе причина.
    async fn send(
        &self,
        req: reqwest::RequestBuilder,
        timeout: Duration,
        what: &'static str,
    ) -> Result<Vec<u8>, BankError> {
        let resp = req
            .header("X-API-KEY", &self.key.0)
            .header("Accept", "application/json")
            .timeout(timeout)
            .send()
            .await
            .map_err(|e| {
                // Текст ошибки сети не берём: в нём бывает адрес; только вид.
                let kind = if e.is_timeout() {
                    "таймаут"
                } else if e.is_connect() {
                    "нет соединения"
                } else {
                    "ошибка запроса"
                };
                tracing::warn!(error = kind, what, "банк: нет связи");
                BankError::Unavailable(kind.into())
            })?;
        let status = resp.status().as_u16();
        let body = resp
            .bytes()
            .await
            .map_err(|_| BankError::Unavailable("ответ оборвался".into()))?
            .to_vec();
        if (200..300).contains(&status) {
            return Ok(body);
        }
        let text = error_text(&body);
        tracing::warn!(status, what, "банк: ошибка");
        Err(match status {
            401 | 403 => BankError::Key(text),
            404 => BankError::NotFound,
            400..=499 => BankError::Rejected(text),
            _ => BankError::Unavailable(format!("банк ответил {status}")),
        })
    }

    fn parse<T: serde::de::DeserializeOwned>(
        body: &[u8],
        what: &'static str,
    ) -> Result<T, BankError> {
        serde_json::from_slice(body).map_err(|_| {
            tracing::warn!(what, "банк: непонятный ответ");
            BankError::Unavailable("непонятный ответ банка".into())
        })
    }

    /// Точки и кассы компании (`GET /api/v2/get-sale-point-infos/{legalPartyId}`).
    pub async fn sale_points(&self, legal_party_id: i64) -> Result<Vec<SalePoint>, BankError> {
        let body = self
            .send(
                self.http.get(format!(
                    "{}/api/v2/get-sale-point-infos/{legal_party_id}",
                    self.url
                )),
                QUICK,
                "точки и кассы",
            )
            .await?;
        let reply: Vec<PointsReply> = Self::parse(&body, "точки и кассы")?;
        Ok(reply
            .into_iter()
            .flat_map(|a| {
                let account = text_of(a.account.as_ref());
                a.sale_point_info_dto_list.into_iter().filter_map(move |p| {
                    Some(SalePoint {
                        account: account.clone(),
                        name: p.name.unwrap_or_default(),
                        address: p.address.unwrap_or_default(),
                        code: code_of(p.code.as_ref())?,
                        cashes: p
                            .cash_dto_list
                            .into_iter()
                            .filter_map(|c| {
                                Some(CashDesk {
                                    name: c.name.unwrap_or_default(),
                                    code: code_of(c.code.as_ref())?,
                                })
                            })
                            .collect(),
                    })
                })
            })
            .collect())
    }

    /// Одноразовый QR на сумму (`POST /api/v2/generate/qr`, `WEB_PARTNER_BY_SALE_POINT`).
    pub async fn generate_qr(&self, q: &QrRequest<'_>) -> Result<NewQr, BankError> {
        let body = GenerateBody {
            requisite: Requisite {
                sale_point_code: q.sale_point_code,
                cash_code: q.cash_code,
                legal_party_id: q.legal_party_id,
            },
            sum: money_json(q.amount).map_err(|_| BankError::Rejected("сумма".into()))?,
            allow_edit_sum: false,
            note: q.note,
            qr_generate_type: "WEB_PARTNER_BY_SALE_POINT",
            transaction_count: 1,
            // Время банка — время точки, без зоны.
            until_date_time: q
                .until
                .with_timezone(&bishkek())
                .format("%Y-%m-%dT%H:%M:%S")
                .to_string(),
            qr_type: "png",
            qr_size: 400,
        };
        let text = serde_json::to_string(&body)
            .map_err(|_| BankError::Rejected("запрос не собран".into()))?;
        let reply = self
            .send(
                self.http
                    .post(format!("{}/api/v2/generate/qr", self.url))
                    .header("Content-Type", "application/json")
                    .body(text),
                QUICK,
                "QR",
            )
            .await?;
        let r: GenerateReply = Self::parse(&reply, "QR")?;
        if r.transaction_id.trim().is_empty() {
            return Err(BankError::Unavailable("банк не прислал номер QR".into()));
        }
        Ok(NewQr {
            transaction_id: r.transaction_id,
            qr_base64: r.qr_base64,
            qr_url: r.qr_url,
        })
    }

    /// Статус QR (`GET /api/v2/get-qr-transaction-info/{id}`); 404 — ещё не оплачен.
    pub async fn qr_status(&self, transaction_id: &str) -> Result<QrState, BankError> {
        let id: String = transaction_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        let body = self
            .send(
                self.http
                    .get(format!("{}/api/v2/get-qr-transaction-info/{id}", self.url)),
                QUICK,
                "статус QR",
            )
            .await?;
        let r: StatusReply = Self::parse(&body, "статус QR")?;
        Ok(QrState {
            status: r.status.trim().to_uppercase(),
            sum: money_of(r.sum.as_deref()),
            operation_id: r.operation_id.filter(|o| !o.trim().is_empty()),
            processed_at: r
                .transaction_processed_date_time
                .as_deref()
                .and_then(parse_bank_time),
        })
    }

    /// Выписка по POS и QR за дни (`GET /api/v1/get-pos-operation-statement`), не больше 14.
    pub async fn pos_statement(
        &self,
        from: NaiveDate,
        to: NaiveDate,
        account: &str,
    ) -> Result<Vec<StatementOp>, BankError> {
        let body = self
            .send(
                self.http
                    .get(format!("{}/api/v1/get-pos-operation-statement", self.url))
                    .query(&[
                        ("startDate", from.format("%Y-%m-%d").to_string()),
                        ("endDate", to.format("%Y-%m-%d").to_string()),
                        ("accountNumber", account.to_string()),
                    ]),
                SLOW,
                "выписка",
            )
            .await?;
        let r: StatementReply = Self::parse(&body, "выписка")?;
        let total = r.pos_operations.len();
        let ops: Vec<StatementOp> = r
            .pos_operations
            .iter()
            .filter_map(|o| statement_op(o))
            .collect();
        if ops.len() < total {
            tracing::warn!(
                skipped = total - ops.len(),
                "банк: операции выписки без времени или суммы пропущены"
            );
        }
        Ok(ops)
    }
}

// ---------- Настройка «Optima Business» ----------

/// Настройка филиала (ключ `optima` в `settings`).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct OptimaSettings {
    /// ID компании в банке (`legalPartyId`).
    #[serde(default)]
    pub legal_party_id: Option<i64>,
    /// Расчётный счёт в банке, 16 цифр — для выписки.
    #[serde(default)]
    pub account_number: String,
    #[serde(default)]
    pub sale_point_code: Option<i64>,
    #[serde(default)]
    pub sale_point_name: String,
    #[serde(default)]
    pub cash_code: Option<i64>,
    #[serde(default)]
    pub cash_name: String,
    /// Безналичный счёт учёта, куда ложатся эти деньги; пусто — счёт для карты по умолчанию.
    #[serde(default)]
    pub cash_account_id: Option<Uuid>,
}

impl OptimaSettings {
    /// Всё для QR: компания, точка и касса.
    pub fn qr_ready(&self) -> bool {
        self.legal_party_id.is_some() && self.sale_point_code.is_some() && self.cash_code.is_some()
    }

    /// Всё для выписки: расчётный счёт.
    pub fn statement_ready(&self) -> bool {
        valid_account_number(&self.account_number)
    }
}

fn valid_account_number(s: &str) -> bool {
    s.len() == 16 && s.bytes().all(|b| b.is_ascii_digit())
}

pub async fn load_settings(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<OptimaSettings> {
    let v = sqlx::query_scalar!(
        "select value from settings where branch_id = $1 and key = 'optima'",
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(v.and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default())
}

/// Счёт учёта для денег Optima: из настройки (если он действует), иначе счёт для карты.
pub async fn money_account(
    conn: &mut PgConnection,
    branch_id: Uuid,
    s: &OptimaSettings,
) -> AppResult<Option<Uuid>> {
    if let Some(id) = s.cash_account_id {
        let ok = sqlx::query_scalar!(
            r#"select exists (select 1 from cash_accounts
                              where id = $1 and branch_id = $2 and kind = 'bank' and active) as "ok!""#,
            id,
            branch_id
        )
        .fetch_one(&mut *conn)
        .await?;
        if ok {
            return Ok(Some(id));
        }
    }
    crate::api::cash::bank_account(conn, branch_id, "card").await
}

#[derive(Serialize)]
struct StatusOut {
    /// На сервере есть ключ банка.
    enabled: bool,
    /// Ключ есть и точка с кассой выбраны: на кассе видна кнопка «QR на сумму».
    qr: bool,
}

async fn status(State(state): State<AppState>, user: CurrentUser) -> AppResult<Json<StatusOut>> {
    if state.optima.is_none() {
        return Ok(Json(StatusOut {
            enabled: false,
            qr: false,
        }));
    }
    let mut conn = state.pool.acquire().await?;
    let s = load_settings(&mut conn, user.branch_id).await?;
    Ok(Json(StatusOut {
        enabled: true,
        qr: s.qr_ready(),
    }))
}

#[derive(Serialize)]
struct SettingsOut {
    /// Ключ `OPTIMA_API_KEY` задан на сервере.
    enabled: bool,
    #[serde(flatten)]
    settings: OptimaSettings,
    /// Счёт учёта, куда пойдут деньги сейчас (с учётом счёта по умолчанию).
    account_id: Option<Uuid>,
}

async fn get_settings(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<SettingsOut>> {
    user.require_owner()?;
    let mut conn = state.pool.acquire().await?;
    let settings = load_settings(&mut conn, user.branch_id).await?;
    let account_id = money_account(&mut conn, user.branch_id, &settings).await?;
    Ok(Json(SettingsOut {
        enabled: state.optima.is_some(),
        settings,
        account_id,
    }))
}

async fn put_settings(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<OptimaSettings>,
) -> AppResult<Json<SettingsOut>> {
    ctx.user.require_owner()?;
    let branch_id = ctx.user.branch_id;
    let mut s = req;
    s.account_number = s.account_number.trim().replace(' ', "");
    if !s.account_number.is_empty() && !valid_account_number(&s.account_number) {
        return Err(invalid("расчётный счёт — 16 цифр"));
    }
    if s.legal_party_id.is_some_and(|v| v <= 0)
        || s.sale_point_code.is_some_and(|v| v < 0)
        || s.cash_code.is_some_and(|v| v < 0)
    {
        return Err(invalid("ID компании, точка и касса — положительные числа"));
    }
    s.sale_point_name = clip(&s.sale_point_name, 200);
    s.cash_name = clip(&s.cash_name, 200);
    let mut tx = state.pool.begin().await?;
    if let Some(id) = s.cash_account_id {
        let kind = sqlx::query_scalar!(
            "select kind from cash_accounts where id = $1 and branch_id = $2 and active",
            id,
            branch_id
        )
        .fetch_optional(&mut *tx)
        .await?;
        if kind.as_deref() != Some("bank") {
            return Err(invalid("деньги Optima ложатся только на безналичный счёт"));
        }
    }
    let before = load_settings(&mut tx, branch_id).await?;
    let value = serde_json::to_value(&s).map_err(|e| AppError::Internal(e.to_string()))?;
    sqlx::query!(
        r#"insert into settings (branch_id, key, value) values ($1, 'optima', $2)
           on conflict (branch_id, key) do update set value = excluded.value"#,
        branch_id,
        value
    )
    .execute(&mut *tx)
    .await?;
    if before != s {
        let old = serde_json::to_value(&before).map_err(|e| AppError::Internal(e.to_string()))?;
        crate::ops::audit(
            &mut tx,
            &ctx,
            "settings.optima",
            "settings",
            None,
            json!({ "changes": crate::ops::changes(&old, &value) }),
        )
        .await?;
    }
    let account_id = money_account(&mut tx, branch_id, &s).await?;
    tx.commit().await?;
    Ok(Json(SettingsOut {
        enabled: state.optima.is_some(),
        settings: s,
        account_id,
    }))
}

fn clip(s: &str, n: usize) -> String {
    crate::api::bot_drafts::one_line(s.trim())
        .chars()
        .take(n)
        .collect()
}

#[derive(Deserialize)]
struct PointsQuery {
    legal_party_id: i64,
}

fn require_bank(state: &AppState) -> AppResult<&Optima> {
    state.optima.as_ref().ok_or_else(|| {
        invalid("ключ Optima Business не задан на сервере (OPTIMA_API_KEY в настройке точки)")
    })
}

/// «Загрузить точки и кассы» — из банка по ID компании.
async fn sale_points(
    State(state): State<AppState>,
    user: CurrentUser,
    Query(q): Query<PointsQuery>,
) -> AppResult<Json<Vec<SalePoint>>> {
    user.require_owner()?;
    let bank = require_bank(&state)?;
    if q.legal_party_id <= 0 {
        return Err(invalid("ID компании — положительное число"));
    }
    match bank.sale_points(q.legal_party_id).await {
        Ok(points) => Ok(Json(points)),
        Err(e) => {
            key_problem(&state.pool, user.branch_id, &e).await;
            Err(e.into())
        }
    }
}

#[derive(Serialize)]
struct CheckOut {
    ok: bool,
    message: String,
}

/// «Проверить связь»: ключ подходит, точка и касса из настройки есть в банке.
async fn check(State(state): State<AppState>, user: CurrentUser) -> AppResult<Json<CheckOut>> {
    user.require_owner()?;
    let bank = require_bank(&state)?;
    let mut conn = state.pool.acquire().await?;
    let s = load_settings(&mut conn, user.branch_id).await?;
    drop(conn);
    let Some(legal) = s.legal_party_id else {
        return Err(invalid("укажите ID компании"));
    };
    let points = match bank.sale_points(legal).await {
        Ok(p) => p,
        Err(e) => {
            key_problem(&state.pool, user.branch_id, &e).await;
            return Ok(Json(CheckOut {
                ok: false,
                message: e.message(),
            }));
        }
    };
    let point = s
        .sale_point_code
        .and_then(|c| points.iter().find(|p| p.code == c));
    let message = match (point, s.cash_code) {
        (None, _) if s.sale_point_code.is_some() => {
            "Связь есть, но выбранной точки в банке нет — загрузите точки заново".to_string()
        }
        (Some(p), Some(c)) if !p.cashes.iter().any(|x| x.code == c) => {
            "Связь есть, но выбранной кассы у точки нет — загрузите точки заново".to_string()
        }
        (Some(p), _) => format!("Связь есть: точка «{}»", p.name),
        _ => format!("Связь есть: точек в банке — {}", points.len()),
    };
    Ok(Json(CheckOut {
        ok: message.starts_with("Связь есть:"),
        message,
    }))
}

/// Ключ банка не подошёл — владельцу «Ключ Optima Business не подходит», не чаще раза в час.
pub async fn key_problem(pool: &PgPool, branch_id: Uuid, e: &BankError) {
    let BankError::Key(text) = e else {
        return;
    };
    let r = sqlx::query!(
        r#"insert into audit_log (id, branch_id, action, entity, data)
           select $1, $2, 'optima.key', 'settings', $3
           where not exists (select 1 from audit_log
                             where branch_id = $2 and action = 'optima.key'
                               and created_at > now() - interval '1 hour')"#,
        crate::ops::new_id(),
        branch_id,
        json!({ "reason": text })
    )
    .execute(pool)
    .await;
    if let Err(e) = r {
        tracing::warn!(error = %e, "банк: уведомление о ключе не записано");
    }
}

// ---------- Фоновая работа ----------

/// Устройство операций, которые сервер проводит сам (сверка в 06:00).
pub const SERVER_DEVICE: Uuid = Uuid::from_u128(0x0b7e_0a11_0000_4000_8000_0000_0000_5e4f);

/// Опрос QR каждые 2 с, «QR оплачен, чек не проведён», сверка в 06:00.
pub fn spawn(pool: PgPool, bank: Optima) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last_daily: Option<std::time::Instant> = None;
        loop {
            tick.tick().await;
            if let Err(e) = crate::api::bank_qr::poll_once(&pool, &bank).await {
                tracing::warn!(error = %e, "банк: опрос QR не прошёл");
            }
            // Раз в минуту: оплаченные QR без чека и сверка дня.
            if last_daily.is_none_or(|t| t.elapsed() >= Duration::from_secs(60)) {
                last_daily = Some(std::time::Instant::now());
                if let Err(e) = crate::api::bank_qr::alert_unclaimed(&pool).await {
                    tracing::warn!(error = %e, "банк: проверка QR без чека не прошла");
                }
                crate::api::bank_recon::daily(&pool, &bank, Utc::now()).await;
            }
        }
    });
    tracing::info!("Optima Business: опрос QR и сверка включены");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bank_sums_to_tyiyn_without_fractions() {
        assert_eq!(parse_money("990.00"), Some(99_000));
        assert_eq!(parse_money("38"), Some(3_800));
        assert_eq!(parse_money("1500.5"), Some(150_050));
        assert_eq!(parse_money("0.00"), Some(0));
        assert_eq!(parse_money("\"12.30\""), Some(1_230));
        assert_eq!(parse_money("-4.95"), Some(-495));
        assert_eq!(parse_money("18698.000"), Some(1_869_800));
        assert_eq!(parse_money("0.005"), None);
        assert_eq!(parse_money("1e3"), None);
        assert_eq!(parse_money(""), None);
        assert_eq!(parse_money("12."), None);
        assert_eq!(parse_money(".5"), None);
    }

    #[test]
    fn sums_to_bank_with_two_digits() {
        assert_eq!(
            money_json(150_050).map(|r| r.get().to_string()).ok(),
            Some("1500.50".into())
        );
        assert_eq!(
            money_json(3_800).map(|r| r.get().to_string()).ok(),
            Some("38.00".into())
        );
        assert_eq!(
            money_json(5).map(|r| r.get().to_string()).ok(),
            Some("0.05".into())
        );
    }

    #[test]
    fn bank_time_without_zone_is_bishkek() {
        let t = parse_bank_time("2025-08-09T00:43:07").map(|t| t.to_rfc3339());
        assert_eq!(t.as_deref(), Some("2025-08-08T18:43:07+00:00"));
        let t = parse_bank_time("2026-02-27T09:48:57.42530573").map(|t| t.to_rfc3339());
        assert_eq!(t.as_deref(), Some("2026-02-27T03:48:57.425305730+00:00"));
        let t = parse_bank_time("2025-09-26T14:18:26.000000Z").map(|t| t.to_rfc3339());
        assert_eq!(t.as_deref(), Some("2025-09-26T14:18:26+00:00"));
    }

    #[test]
    fn statement_operation_keeps_exact_sums() {
        let raw = RawValue::from_string(
            r#"{"operationProcessedDateTime":"2025-08-09T00:43:07","posTerminalCode":"PR900482",
                "authCode":"530248","requisite":"499001******6776","operationSum":990.00,
                "operationFeePercent":0.50,"operationFeeSum":4.95,"operationTransferSum":985.05,
                "currency":"KGS","rrn":"123456789012"}"#
                .to_string(),
        )
        .ok();
        let op = raw.as_deref().and_then(statement_op);
        let op = op.as_ref();
        assert_eq!(op.map(|o| o.amount), Some(99_000));
        assert_eq!(op.map(|o| o.fee), Some(495));
        assert_eq!(op.map(|o| o.transfer), Some(98_505));
        assert_eq!(op.map(|o| o.kind), Some("card"));
        assert_eq!(op.map(|o| o.key.as_str()), Some("rrn:123456789012"));
        assert!(op.is_some_and(|o| o.raw.contains("990.00")));
    }
}
