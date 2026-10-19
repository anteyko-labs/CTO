//! Операции владельца через бота (SPEC-24, ADR-068): черновик, проверка, проведение кнопкой.
//!
//! Разбор текста предлагает операцию полями «как сказал владелец»: суммы и количества — строкой,
//! статьи, кассы, клиенты, сотрудники и товары — названием. Здесь сервер разбирает суммы той же
//! функцией, что экраны (инвариант 1), находит объекты теми же поисками, что экраны, и проверяет
//! операцию той же функцией, которой её проводит программа, — в транзакции, которая всегда
//! откатывается. Прошедший проверку черновик сохраняется в `bot_drafts` с готовым `op_id` и
//! показывается владельцу целиком с кнопками «✅ Провести» и «✖ Отмена». Проводится только
//! кнопкой: снова той же функцией, от имени владельца, с `op_id` черновика.

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Connection, PgConnection};
use uuid::Uuid;

use crate::api::cash::{MovementReq, TransferReq};
use crate::api::catalog::{PricesReq, ProductFilter, ProductOut, query_products};
use crate::api::expenses::ExpenseReq;
use crate::api::parties::{AdjustReq, ListQuery, PartyOut, RepaymentReq, search_parties};
use crate::api::payroll::{AccrualReq, PayoutReq, SalaryReq};
use crate::api::receipts::{PurchasePriceReq, ReceiptLineReq, ReceiptReq};
use crate::auth::{Ctx, CurrentUser, Role};
use crate::domain::money::{format_som, format_som_signed, parse_liters, parse_spoken_som};
use crate::error::{AppError, AppResult};
use crate::ops;

/// Черновик живёт 10 минут.
pub const DRAFT_MINUTES: i32 = 10;
/// Не больше 30 проведённых через бот операций в сутки на филиал.
pub const DAILY_LIMIT: i64 = 30;
/// Строк в приходе через бот — не больше.
const MAX_LINES: usize = 30;
/// Названия и параметры — не длиннее, комментарии обрезаются.
const MAX_NAME: usize = 100;
const MAX_COMMENT: usize = 200;

pub const LIMIT_TEXT: &str =
    "Через бот сегодня уже проведено 30 операций — остальное внесите в программе.";
const NOT_YOURS: &str = "Черновик не найден — ничего не проведено.";

/// Операция черновика: запрос той же функции, которой её проводит программа.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum DraftOp {
    Expense(ExpenseReq),
    CashMovement(MovementReq),
    CashTransfer(TransferReq),
    Repayment(RepaymentReq),
    DebtAdjust(AdjustReq),
    Accrual(AccrualReq),
    Payout(PayoutReq),
    Salary(SalaryReq),
    Prices {
        product_id: Uuid,
        req: PricesReq,
    },
    PurchasePrice {
        product_id: Uuid,
        req: PurchasePriceReq,
    },
    Receipt(ReceiptReq),
}

/// Что получилось при проведении: строка для владельца и номер документа, если он есть.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Posted {
    pub text: String,
    pub number: Option<i64>,
}

/// Черновик, показанный владельцу.
#[derive(Debug, Clone)]
pub struct Shown {
    pub id: Uuid,
    pub summary: String,
}

/// Итог подготовки черновика: показан или нет (с объяснением для разбора текста).
#[derive(Debug)]
pub enum Prepared {
    Draft(Shown),
    Refused(String),
}

/// Кнопки под черновиком.
pub fn buttons(id: Uuid) -> Vec<(String, String)> {
    vec![
        ("✅ Провести".into(), format!("d:ok:{id}")),
        ("✖ Отмена".into(), format!("d:no:{id}")),
    ]
}

/// Проводит операцию той же функцией, что программа. Вызывается внутри транзакции.
pub async fn execute(conn: &mut PgConnection, ctx: &Ctx, op: DraftOp) -> AppResult<Posted> {
    Ok(match op {
        DraftOp::Expense(req) => {
            let out = crate::api::expenses::post_expense_tx(conn, ctx, req).await?;
            Posted {
                text: format!(
                    "расход № {} — {}, {}",
                    out.number,
                    out.article_name,
                    format_som(out.amount_tyiyn)
                ),
                number: Some(out.number),
            }
        }
        DraftOp::CashMovement(req) => {
            let what = if req.kind == "cash_in" {
                "внесение"
            } else {
                "изъятие"
            };
            let amount = req.amount_tyiyn;
            let out = crate::api::cash::post_movement_tx(conn, ctx, req).await?;
            Posted {
                text: format!(
                    "{what} {}, в кассе теперь {}",
                    format_som(amount),
                    format_som(out.balance_tyiyn)
                ),
                number: None,
            }
        }
        DraftOp::CashTransfer(req) => {
            let amount = req.amount_tyiyn;
            let out = crate::api::cash::post_transfer_tx(conn, ctx, req).await?;
            let number = out.get("number").and_then(Value::as_i64);
            Posted {
                text: format!(
                    "перемещение № {} — {}",
                    number.unwrap_or_default(),
                    format_som(amount)
                ),
                number,
            }
        }
        DraftOp::Repayment(req) => {
            let amount = req.amount_tyiyn;
            let out = crate::api::parties::post_repayment_tx(conn, ctx, req).await?;
            Posted {
                text: format!(
                    "оплата {} учтена, баланс теперь {}",
                    format_som(amount),
                    balance_words(out.balance_tyiyn)
                ),
                number: None,
            }
        }
        DraftOp::DebtAdjust(req) => {
            let out = crate::api::parties::post_adjust_tx(conn, ctx, req).await?;
            Posted {
                text: format!(
                    "долг перенесён, баланс теперь {}",
                    balance_words(out.balance_tyiyn)
                ),
                number: None,
            }
        }
        DraftOp::Accrual(req) => {
            let what = if req.kind == "bonus" {
                "премия"
            } else {
                "удержание"
            };
            let amount = req.amount_tyiyn;
            crate::api::payroll::post_accrual_tx(conn, ctx, req).await?;
            Posted {
                text: format!("{what} {}", format_som(amount)),
                number: None,
            }
        }
        DraftOp::Payout(req) => {
            let what = if req.advance {
                "аванс"
            } else {
                "выплата"
            };
            let amount = req.amount_tyiyn;
            let out = crate::api::payroll::post_payout_tx(conn, ctx, req).await?;
            Posted {
                text: format!("{what} № {} — {}", out.number, format_som(amount)),
                number: Some(out.number),
            }
        }
        DraftOp::Salary(req) => {
            let month = req.month;
            crate::api::payroll::post_salary_tx(conn, ctx, req).await?;
            Posted {
                text: format!("оклад за {}", month_name(month)),
                number: None,
            }
        }
        DraftOp::Prices { product_id, req } => {
            let out = crate::api::catalog::update_prices_tx(conn, ctx, product_id, req).await?;
            Posted {
                text: format!(
                    "«{}»: цена {}, минимальный остаток {}",
                    out.name,
                    format_som(out.sale_price_tyiyn),
                    qty_text(out.min_stock, &out.unit)
                ),
                number: None,
            }
        }
        DraftOp::PurchasePrice { product_id, req } => {
            let price = req.price_tyiyn;
            let out = crate::api::receipts::purchase_price_tx(conn, ctx, product_id, req).await?;
            Posted {
                text: format!(
                    "цена закупки {}, строк чеков получили себестоимость: {}",
                    format_som(price),
                    out.lines
                ),
                number: None,
            }
        }
        DraftOp::Receipt(req) => {
            let out = crate::api::receipts::post_receipt_tx(conn, ctx, req).await?;
            Posted {
                text: format!("приход № {} — {}", out.number, format_som(out.total_tyiyn)),
                number: Some(out.number),
            }
        }
    })
}

fn balance_words(b: i64) -> String {
    match b {
        0 => "0 с — долга нет".into(),
        b if b > 0 => format!("{} — должны нам", format_som(b)),
        b => format!("{} — должны мы", format_som(-b)),
    }
}

/// Причина отказа для владельца; сбой базы — ошибкой дальше.
fn reason(e: AppError) -> AppResult<String> {
    match e {
        AppError::Validation(m)
        | AppError::Conflict(m)
        | AppError::TooManyRequests(m)
        | AppError::ForbiddenWith(m) => Ok(m),
        AppError::Forbidden => Ok("недостаточно прав".into()),
        AppError::NotFound => Ok("не найдено".into()),
        AppError::Unauthorized => Ok("требуется вход".into()),
        other => Err(other),
    }
}

/// Владелец, к которому привязан чат: только активный владелец.
async fn chat_owner(conn: &mut PgConnection, chat_id: i64) -> AppResult<Option<CurrentUser>> {
    Ok(linked_owner(conn, chat_id).await?.map(|(u, _)| u))
}

/// Владелец чата и номер в Телеграме того, кто привязал чат (ADR-070).
async fn linked_owner(
    conn: &mut PgConnection,
    chat_id: i64,
) -> AppResult<Option<(CurrentUser, Option<i64>)>> {
    Ok(sqlx::query!(
        r#"select u.id, u.branch_id, u.login, u.full_name, t.tg_user_id
           from telegram_links t join users u on u.id = t.user_id
           where t.chat_id = $1 and u.active and u.role = 'owner'"#,
        chat_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .map(|r| {
        (
            CurrentUser {
                id: r.id,
                branch_id: r.branch_id,
                login: r.login,
                full_name: r.full_name,
                role: Role::Owner,
            },
            r.tg_user_id,
        )
    }))
}

/// Закрывает открытый черновик чата (новый черновик, «✖ Закончить», режим закрыт): просроченный —
/// «истёк», остальной — «отменён». Строка черновика только завершается (SPEC-24).
pub(crate) async fn close_open(conn: &mut PgConnection, chat_id: i64, why: &str) -> AppResult<()> {
    sqlx::query!(
        r#"update bot_drafts
           set status = case when expires_at <= now() then 'expired' else 'cancelled' end,
               finished_at = now(),
               result = jsonb_build_object('reason', $2::text)
           where chat_id = $1 and status = 'open'"#,
        chat_id,
        why
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Черновики, срок которых прошёл, а кнопку так и не нажали, помечаются «истёк» — фоновым
/// проходом бота (SPEC-24). Возвращает, сколько закрыто.
pub async fn expire_drafts(conn: &mut PgConnection) -> AppResult<u64> {
    Ok(sqlx::query!(
        r#"update bot_drafts
           set status = 'expired', finished_at = now(),
               result = jsonb_build_object('reason', 'прошло 10 минут')
           where status = 'open' and expires_at <= now()"#
    )
    .execute(&mut *conn)
    .await?
    .rows_affected())
}

/// Сколько операций через бот проведено сегодня — под блокировкой филиала: два нажатия
/// одновременно не превысят предел (ADR-070). Блокировка держится до конца транзакции.
async fn posted_today_locked(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<i64> {
    sqlx::query!(
        "select pg_advisory_xact_lock(hashtextextended($1::text, 7))",
        format!("bot_daily:{branch_id}")
    )
    .execute(&mut *conn)
    .await?;
    posted_today(conn, branch_id).await
}

/// Сколько операций проведено через бот сегодня (по Бишкеку) в филиале.
async fn posted_today(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<i64> {
    Ok(sqlx::query_scalar!(
        r#"select count(*) as "n!" from bot_drafts
           where branch_id = $1 and status = 'posted'
             and finished_at >= ((now() at time zone 'Asia/Bishkek')::date)::timestamp at time zone 'Asia/Bishkek'"#,
        branch_id
    )
    .fetch_one(&mut *conn)
    .await?)
}

/// Черновик готовят только владельцу этого чата в открытом режиме «💬 Спросить» (SPEC-24):
/// режим и так пускает сюда только его, но сервер проверяет ещё раз.
async fn may_draft(conn: &mut PgConnection, chat_id: i64, user: &CurrentUser) -> AppResult<bool> {
    let owner = chat_owner(conn, chat_id).await?;
    if owner.is_none_or(|o| o.id != user.id || o.branch_id != user.branch_id) {
        return Ok(false);
    }
    Ok(sqlx::query_scalar!(
        r#"select exists (select 1 from bot_question_mode
                          where chat_id = $1 and user_id = $2 and branch_id = $3
                            and last_at > now() - interval '10 minutes') as "e!""#,
        chat_id,
        user.id,
        user.branch_id
    )
    .fetch_one(&mut *conn)
    .await?)
}

// ---------- Разбор полей ----------

pub(crate) fn norm(s: &str) -> String {
    s.trim().to_lowercase().replace('ё', "е")
}

/// Строковое поле: без пробелов по краям, пустое — `None`, длинное — отказ.
fn field(input: &Value, key: &str) -> Result<Option<String>, String> {
    let Some(v) = input.get(key) else {
        return Ok(None);
    };
    let s = match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        Value::Null => return Ok(None),
        _ => return Err(format!("поле {key} — строкой")),
    };
    if s.is_empty() {
        return Ok(None);
    }
    if s.chars().count() > MAX_NAME {
        return Err(format!("поле {key} не длиннее {MAX_NAME} знаков"));
    }
    Ok(Some(s))
}

fn need(input: &Value, key: &str, what: &str) -> Result<String, String> {
    field(input, key)?.ok_or_else(|| format!("не указано: {what} (поле {key})"))
}

fn comment(input: &Value, key: &str) -> String {
    one_line(input.get(key).and_then(Value::as_str).unwrap_or(""))
        .chars()
        .take(MAX_COMMENT)
        .collect()
}

/// Сумма строкой, как сказал владелец: «1 500», «1500,50», «1 500 сом».
fn amount(input: &Value, key: &str) -> Result<i64, String> {
    let s = need(input, key, "сумма")?;
    let v = parse_spoken_som(&s).ok_or_else(|| {
        format!("сумму «{s}» не разобрать: нужны цифры, например «1 500» или «350,50»")
    })?;
    if v <= 0 {
        return Err("сумма больше нуля".into());
    }
    ops::check_amount(v).map_err(|e| e.to_string())?;
    Ok(v)
}

/// Текст для черновика одной строкой: переводы строк и прочие управляющие знаки (и знаки,
/// переворачивающие направление текста) заменяются пробелом — иначе комментарий вида
/// «—\n\nИтого: 100,00 с» дописал бы в черновик чужие строки (ADR-070).
pub(crate) fn one_line(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_control()
                || matches!(
                    c,
                    '\u{2028}' | '\u{2029}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}'
                        | '\u{2066}'..='\u{2069}'
                )
            {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Свободный текст владельца в черновике — в кавычках «…», пустой — «—».
fn quoted(s: &str) -> String {
    let s = one_line(s);
    if s.is_empty() {
        "—".into()
    } else {
        format!("«{s}»")
    }
}

/// Значащие части названия: слова от 3 букв без предлогов, первые 4 буквы — так «кассы»
/// находит «Касса», «Бакыту» — «Бакыт», «хозтовары» — «Хозтовары».
fn stems(s: &str) -> Vec<String> {
    norm(s)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3 && !matches!(*w, "для" | "под" | "над" | "без"))
        .map(|w| w.chars().take(4).collect())
        .collect()
}

/// Совпадает ли название с тем, что сказал владелец: все значащие части есть в начале слов.
pub(crate) fn sounds_like(name: &str, q: &str) -> bool {
    let words: Vec<String> = norm(name)
        .split(|c: char| !c.is_alphanumeric())
        .map(str::to_string)
        .collect();
    let want = stems(q);
    !want.is_empty()
        && want
            .iter()
            .all(|st| words.iter().any(|w| w.starts_with(st.as_str())))
}

/// Выбор одного из найденного: один — он; несколько, но ровно одно точное совпадение — оно;
/// иначе — список для уточнения или «не найдено». Черновик в этих случаях не создаётся.
fn choose<T>(
    found: Vec<T>,
    label: impl Fn(&T) -> String,
    exact: impl Fn(&T) -> bool,
    none: String,
    many: String,
) -> Result<T, String> {
    let total = found.len();
    let exact_at: Vec<usize> = found
        .iter()
        .enumerate()
        .filter(|(_, x)| exact(x))
        .map(|(i, _)| i)
        .collect();
    let pick = match (total, exact_at.as_slice()) {
        (0, _) => return Err(none),
        (1, _) => Some(0),
        (_, [i]) => Some(*i),
        _ => None,
    };
    if let Some(i) = pick {
        return found.into_iter().nth(i).ok_or(none);
    }
    let mut lines = vec![format!("{many} (найдено {total}):")];
    for x in found.iter().take(10) {
        lines.push(format!("— {}", one_line(&label(x))));
    }
    if total > 10 {
        lines.push(format!("и ещё {}", total - 10));
    }
    lines.push("Черновик не создан — уточните, какой нужен.".into());
    Err(lines.join("\n"))
}

/// Откуда деньги: касса или счёт филиала. «Из своих, мимо касс» больше нет (ADR-079).
#[derive(Debug, Clone)]
enum Source {
    Account {
        id: Uuid,
        name: String,
        balance: i64,
    },
}

impl Source {
    fn line(&self) -> String {
        match self {
            Source::Account { name, balance, .. } => {
                format!("{} (сейчас {})", one_line(name), format_som(*balance))
            }
        }
    }
}

fn is_outside(q: &str) -> bool {
    let n = norm(q);
    n.contains("свои") || n.contains("своих") || n.contains("мимо касс") || n == "outside"
}

/// Касса по названию («Касса», «Счёт», «Сейф владельца»); пусто — касса по умолчанию.
/// «Из своих» — отказ с объяснением: так больше не платят (ADR-079).
async fn account(
    conn: &mut PgConnection,
    branch_id: Uuid,
    q: Option<&str>,
) -> AppResult<Result<Source, String>> {
    crate::api::cash::ensure_accounts(conn, branch_id).await?;
    if let Some(q) = q
        && is_outside(q)
    {
        return Ok(Err(crate::api::receipts::NO_OUTSIDE.into()));
    }
    let all = sqlx::query!(
        r#"select id, name, kind, balance_tyiyn, is_default from cash_accounts
           where branch_id = $1 and active order by is_default desc, kind, name"#,
        branch_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let found: Vec<_> = match q {
        // Без названия — касса смены (`is_default` — только у неё, ADR-077).
        None => all
            .into_iter()
            .filter(|a| a.is_default && a.kind == "register")
            .collect(),
        Some(q) => {
            let wanted = norm(q);
            if all.iter().any(|a| norm(&a.name) == wanted) {
                all.into_iter()
                    .filter(|a| norm(&a.name) == wanted)
                    .collect()
            } else {
                all.into_iter()
                    .filter(|a| sounds_like(&a.name, q))
                    .collect()
            }
        }
    };
    let asked = q.unwrap_or("Касса").to_string();
    Ok(choose(
        found,
        |a| format!("{}: {}", a.name, format_som(a.balance_tyiyn)),
        |_| false,
        format!(
            "Касса «{asked}» не найдена. Кассы: «Касса», «Счёт», «Сейф магазина», «Сейф владельца»."
        ),
        format!("По «{asked}» подходит несколько касс"),
    )
    .map(|a| Source::Account {
        id: a.id,
        name: a.name,
        balance: a.balance_tyiyn,
    }))
}

/// Касса по номеру — для показа в черновике.
async fn account_by_id(conn: &mut PgConnection, id: Uuid) -> AppResult<Source> {
    let a = sqlx::query!(
        "select name, balance_tyiyn from cash_accounts where id = $1",
        id
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(Source::Account {
        id,
        name: a.name,
        balance: a.balance_tyiyn,
    })
}

/// Контрагент по названию, телефону или госномеру — тем же поиском, что экран «Клиенты».
pub(crate) async fn find_party(
    conn: &mut PgConnection,
    branch_id: Uuid,
    role: Option<&str>,
    q: &str,
) -> AppResult<Result<PartyOut, String>> {
    let search = |q: &str| ListQuery {
        role: role.map(str::to_string),
        q: Some(q.to_string()),
        ..Default::default()
    };
    let mut found = search_parties(conn, branch_id, search(q)).await?;
    // «Азамату», «Автоснабу»: без последней буквы.
    if found.is_empty() && q.chars().count() > 4 {
        let shorter: String = q.chars().take(q.chars().count() - 1).collect();
        found = search_parties(conn, branch_id, search(&shorter)).await?;
    }
    found.retain(|p| p.active);
    let who = match role {
        Some("supplier") => "Поставщик",
        Some("customer") => "Клиент",
        _ => "Клиент или поставщик",
    };
    let wanted = norm(q);
    Ok(choose(
        found,
        |p| format!("{}: {}", p.name, balance_words(p.balance_tyiyn)),
        |p| norm(&p.name) == wanted,
        format!("{who} «{q}» не найден. Черновик не создан."),
        format!("По «{q}» найдено несколько: {}", who.to_lowercase()),
    ))
}

struct Employee {
    id: Uuid,
    name: String,
}

async fn employee(
    conn: &mut PgConnection,
    branch_id: Uuid,
    q: &str,
) -> AppResult<Result<Employee, String>> {
    let all = sqlx::query!(
        "select id, full_name from employees where branch_id = $1 and active order by full_name",
        branch_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let wanted = norm(q);
    let found: Vec<Employee> = all
        .into_iter()
        .filter(|e| norm(&e.full_name) == wanted || sounds_like(&e.full_name, q))
        .map(|e| Employee {
            id: e.id,
            name: e.full_name,
        })
        .collect();
    Ok(choose(
        found,
        |e| e.name.clone(),
        |e| norm(&e.name) == wanted,
        format!("Сотрудник «{q}» не найден. Черновик не создан."),
        format!("По «{q}» подходит несколько сотрудников"),
    ))
}

/// Товар по названию, бренду, артикулу или штрихкоду — тем же поиском, что склад.
async fn product(
    conn: &mut PgConnection,
    user: &CurrentUser,
    q: &str,
) -> AppResult<Result<ProductOut, String>> {
    let found = query_products(
        conn,
        user,
        ProductFilter {
            q: Some(q.to_string()),
            limit: 20,
            ..Default::default()
        },
    )
    .await?;
    let wanted = norm(q);
    Ok(choose(
        found,
        |p| {
            [p.name.as_str(), p.brand.as_str(), p.article.as_str()]
                .into_iter()
                .filter(|x| !x.is_empty())
                .collect::<Vec<_>>()
                .join(", ")
        },
        |p| {
            norm(&p.name) == wanted
                || (!p.article.is_empty() && norm(&p.article) == wanted)
                || p.barcodes.iter().any(|b| norm(b) == wanted)
        },
        format!(
            "Товар «{q}» не найден. Новый товар через бот не заводится — заведите его в программе."
        ),
        format!("По «{q}» найдено несколько товаров"),
    ))
}

/// Количество строки прихода: целое число и, по желанию, единица — «10», «10 шт», «3 кан.»,
/// «3 канистры». Любые другие буквы («10к») — не догадываемся, просим уточнить (ADR-070).
fn receipt_count(s: &str) -> Option<i64> {
    let s = s.trim();
    let digits_end = s
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_digit() || c.is_whitespace()))
        .map_or(s.len(), |(i, _)| i);
    let (num, unit) = s.split_at(digits_end);
    let unit = norm(unit);
    let unit = unit.trim_end_matches('.');
    if !matches!(
        unit,
        "" | "шт" | "штук" | "штуки" | "штука" | "кан" | "канистр" | "канистра" | "канистры"
    ) {
        return None;
    }
    num.replace(char::is_whitespace, "")
        .parse::<i64>()
        .ok()
        .filter(|v| *v > 0)
}

/// Количество для текста бота. Знак минуса не теряется и у долей литра: −500 мл → «−0,5 л».
pub(crate) fn qty_text(qty: i64, unit: &str) -> String {
    crate::domain::qty::qty_text(qty, unit)
}

/// За что цена товара: за штуку, за канистру, за килограмм.
fn per_text(p: &ProductOut) -> String {
    match (p.unit.as_str(), p.container_ml) {
        ("g", _) => "за кг".into(),
        (_, Some(ml)) => format!("за канистру {}", qty_text(ml, "ml")),
        _ => "за шт".into(),
    }
}

const MONTHS: [&str; 12] = [
    "январь",
    "февраль",
    "март",
    "апрель",
    "май",
    "июнь",
    "июль",
    "август",
    "сентябрь",
    "октябрь",
    "ноябрь",
    "декабрь",
];

fn month_name(d: NaiveDate) -> String {
    format!(
        "{} {}",
        MONTHS.get(d.month0() as usize).copied().unwrap_or(""),
        d.year()
    )
}

async fn today(conn: &mut PgConnection) -> AppResult<NaiveDate> {
    Ok(
        sqlx::query_scalar!(r#"select (now() at time zone 'Asia/Bishkek')::date as "d!""#)
            .fetch_one(&mut *conn)
            .await?,
    )
}

// ---------- Подготовка черновика ----------

/// Черновик операции из полей, которые распознал разбор текста. `tool` — какой черновик:
/// `draft_expense`, `draft_money`, `draft_debt`, `draft_staff`, `draft_product`, `draft_receipt`.
/// `Err` — только сбой базы; всё остальное — `Prepared::Refused` с объяснением.
pub async fn prepare(
    conn: &mut PgConnection,
    user: &CurrentUser,
    chat_id: i64,
    tool: &str,
    input: &Value,
) -> AppResult<Prepared> {
    if user.role != Role::Owner || !may_draft(conn, chat_id, user).await? {
        return Ok(Prepared::Refused(
            "Черновики операций готовятся только владельцу в режиме «💬 Спросить».".into(),
        ));
    }
    if posted_today(conn, user.branch_id).await? >= DAILY_LIMIT {
        return Ok(Prepared::Refused(LIMIT_TEXT.into()));
    }
    let op_id = ops::new_id();
    let built = match tool {
        "draft_expense" => expense(conn, user, op_id, input).await?,
        "draft_money" => money(conn, user, op_id, input).await?,
        "draft_debt" => debt(conn, user, op_id, input).await?,
        "draft_staff" => staff(conn, user, op_id, input).await?,
        "draft_product" => goods(conn, user, op_id, input).await?,
        "draft_receipt" => receipt(conn, user, op_id, input).await?,
        _ => Err("такого черновика нет".to_string()),
    };
    let built = match built {
        Ok(b) => b,
        Err(why) => return Ok(Prepared::Refused(why)),
    };
    // Проверка той же функцией, что проводит, — в транзакции, которая всегда откатывается.
    let ctx = Ctx::bot(user.clone(), chat_id);
    let mut tx = conn.begin().await?;
    let checked = execute(&mut tx, &ctx, built.op.clone()).await;
    tx.rollback().await?;
    if let Err(e) = checked {
        let why = reason(e)?;
        return Ok(Prepared::Refused(format!(
            "Черновик не создан: {why}.{}",
            built.hint
        )));
    }
    let payload = serde_json::to_value(&built.op).map_err(|e| AppError::Internal(e.to_string()))?;
    let id = ops::new_id();
    let mut tx = conn.begin().await?;
    // У чата один черновик: новый закрывает прежний.
    close_open(&mut tx, chat_id, "заменён новым черновиком").await?;
    sqlx::query!(
        r#"insert into bot_drafts (id, chat_id, user_id, branch_id, kind, payload, summary, op_id,
                                   expires_at)
           values ($1, $2, $3, $4, $5, $6, $7, $8, now() + make_interval(mins => $9))"#,
        id,
        chat_id,
        user.id,
        user.branch_id,
        built.kind,
        payload,
        built.summary,
        op_id,
        DRAFT_MINUTES
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Prepared::Draft(Shown {
        id,
        summary: built.summary,
    }))
}

/// Черновик до проверки: операция, вид, текст для владельца и подсказка к отказу.
struct Built {
    op: DraftOp,
    kind: &'static str,
    summary: String,
    hint: String,
}

type Step = AppResult<Result<Built, String>>;

/// Разобрать поле; ошибка разбора — отказ с объяснением.
macro_rules! take {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(why) => return Ok(Err(why)),
        }
    };
}

fn balance_hint(src: &Source) -> String {
    match src {
        Source::Account { name, balance, .. } => {
            format!(" В «{name}» сейчас {}.", format_som(*balance))
        }
    }
}

async fn expense(conn: &mut PgConnection, user: &CurrentUser, op_id: Uuid, input: &Value) -> Step {
    let branch_id = user.branch_id;
    let article_q = take!(need(input, "article", "статья расхода"));
    let sum = take!(amount(input, "amount"));
    let src_q = take!(field(input, "source"));
    let src = take!(account(conn, branch_id, src_q.as_deref()).await?);
    let today = today(conn).await?;
    let date = match take!(field(input, "date")) {
        None => today,
        Some(s) => take!(
            NaiveDate::parse_from_str(&s, "%Y-%m-%d")
                .map_err(|_| format!("дату «{s}» не разобрать: нужна ГГГГ-ММ-ДД"))
        ),
    };
    if date > today || (today - date).num_days() > 31 {
        return Ok(Err(
            "дата расхода — сегодня или не раньше 31 дня назад".into()
        ));
    }
    crate::api::expenses::ensure_articles(conn, branch_id).await?;
    let all = sqlx::query!(
        "select id, name from expense_articles where branch_id = $1 and active order by name",
        branch_id
    )
    .fetch_all(&mut *conn)
    .await?;
    let wanted = norm(&article_q);
    let found: Vec<_> = all
        .into_iter()
        .filter(|a| norm(&a.name) == wanted || sounds_like(&a.name, &article_q))
        .collect();
    let names = sqlx::query_scalar!(
        "select name from expense_articles where branch_id = $1 and active order by name",
        branch_id
    )
    .fetch_all(&mut *conn)
    .await?
    .join(", ");
    let article = take!(choose(
        found,
        |a| a.name.clone(),
        |a| norm(&a.name) == wanted,
        format!("Статья «{article_q}» не найдена. Статьи: {names}."),
        format!("По «{article_q}» подходит несколько статей"),
    ));
    let text = comment(input, "comment");
    let (source, account_id) = match &src {
        Source::Account { id, .. } => ("account", Some(*id)),
    };
    let summary = format!(
        "Расход\nСтатья: {}\nСумма: {}\nОткуда: {}\nДата: {}\nКомментарий: {}",
        one_line(&article.name),
        format_som(sum),
        src.line(),
        date.format("%d.%m.%Y"),
        quoted(&text)
    );
    Ok(Ok(Built {
        op: DraftOp::Expense(ExpenseReq {
            op_id,
            article_id: article.id,
            amount_tyiyn: sum,
            source: Some(source.into()),
            account_id,
            expense_date: Some(date),
            comment: text,
        }),
        kind: "expense",
        summary,
        hint: balance_hint(&src),
    }))
}

async fn money(conn: &mut PgConnection, user: &CurrentUser, op_id: Uuid, input: &Value) -> Step {
    let branch_id = user.branch_id;
    let operation = norm(&take!(need(input, "operation", "что сделать")));
    let sum = take!(amount(input, "amount"));
    let text = comment(input, "comment");
    if operation.starts_with("внес") || operation.starts_with("изъ") {
        let cash_in = operation.starts_with("внес");
        let acc_q = take!(field(input, "account"));
        let src = take!(account(conn, branch_id, acc_q.as_deref()).await?);
        let Source::Account { id, .. } = &src;
        let summary = format!(
            "{}\nКасса: {}\nСумма: {}\nКомментарий: {}",
            if cash_in {
                "Внесение денег"
            } else {
                "Изъятие денег"
            },
            src.line(),
            format_som(sum),
            quoted(&text)
        );
        return Ok(Ok(Built {
            op: DraftOp::CashMovement(MovementReq {
                op_id,
                account_id: Some(*id),
                kind: if cash_in { "cash_in" } else { "cash_out" }.into(),
                amount_tyiyn: sum,
                comment: text,
            }),
            kind: if cash_in { "cash_in" } else { "cash_out" },
            summary,
            hint: balance_hint(&src),
        }));
    }
    if operation.starts_with("перев")
        || operation.starts_with("перем")
        || operation.contains("сдать")
    {
        let from_q = take!(field(input, "from"));
        let to_q = take!(field(input, "to"));
        let from = take!(account(conn, branch_id, from_q.as_deref()).await?);
        let Some(to_q) = to_q else {
            return Ok(Err("не указано, куда перевести (поле to)".into()));
        };
        let to = take!(account(conn, branch_id, Some(&to_q)).await?);
        let (Source::Account { id: from_id, .. }, Source::Account { id: to_id, .. }) = (&from, &to);
        let summary = format!(
            "Перемещение денег\nОткуда: {}\nКуда: {}\nСумма: {}\nКомментарий: {}",
            from.line(),
            to.line(),
            format_som(sum),
            quoted(&text)
        );
        return Ok(Ok(Built {
            op: DraftOp::CashTransfer(TransferReq {
                op_id,
                from_account_id: *from_id,
                to_account_id: *to_id,
                amount_tyiyn: sum,
                comment: text,
            }),
            kind: "transfer",
            summary,
            hint: balance_hint(&from),
        }));
    }
    Ok(Err(
        "операция с деньгами: «внесение», «изъятие» или «перевод»".into(),
    ))
}

async fn debt(conn: &mut PgConnection, user: &CurrentUser, op_id: Uuid, input: &Value) -> Step {
    let branch_id = user.branch_id;
    let operation = norm(&take!(need(input, "operation", "что сделать")));
    let who = take!(need(input, "party", "клиент или поставщик"));
    let sum = take!(amount(input, "amount"));
    let text = comment(input, "comment");
    if operation.contains("тетрад") || operation.contains("перенос") {
        let p = take!(find_party(conn, branch_id, None, &who).await?);
        let supplier = p.role == "supplier";
        // Клиент должен нам — плюс; мы должны поставщику — минус (SPEC-10).
        let signed = if supplier { -sum } else { sum };
        let reason_text = if text.is_empty() {
            "перенос долга из тетради".to_string()
        } else {
            format!("перенос долга из тетради: {text}")
        };
        let summary = format!(
            "Перенос долга из тетради\n{}: {} (сейчас {})\nСумма: {} ({})\nКомментарий: {}",
            if supplier {
                "Поставщик"
            } else {
                "Клиент"
            },
            one_line(&p.name),
            balance_words(p.balance_tyiyn),
            format_som_signed(signed),
            if supplier {
                "мы должны поставщику"
            } else {
                "клиент должен нам"
            },
            quoted(&reason_text)
        );
        return Ok(Ok(Built {
            op: DraftOp::DebtAdjust(AdjustReq {
                op_id,
                party_id: p.id,
                amount_tyiyn: signed,
                comment: reason_text,
            }),
            kind: "debt_adjust",
            summary,
            hint: String::new(),
        }));
    }
    let supplier = operation.contains("постав");
    if !supplier && !operation.contains("погаш") && !operation.contains("вернул") {
        return Ok(Err(
            "операция с долгом: «погашение долга клиентом», «оплата поставщику» или «перенос долга из тетради»".into(),
        ));
    }
    let p = take!(
        find_party(
            conn,
            branch_id,
            Some(if supplier { "supplier" } else { "customer" }),
            &who
        )
        .await?
    );
    let method_q = norm(&take!(field(input, "method")).unwrap_or_default());
    let method = if method_q.is_empty() || method_q.starts_with("нал") || method_q.contains("касс")
    {
        "cash"
    } else if method_q.starts_with("карт") {
        "card"
    } else if method_q.starts_with("перев") || method_q.contains("сч") {
        "transfer"
    } else if is_outside(&method_q) {
        return Ok(Err(crate::api::receipts::NO_OUTSIDE.into()));
    } else {
        return Ok(Err("чем платят: «наличные», «карта» или «перевод»".into()));
    };
    // Касса — ровно та, которую возьмёт проведение (та же функция), с комиссией банка.
    // Карта и перевод — на счёт по умолчанию для этого способа, комиссия — по его ставке (ADR-077).
    let (src, fee) = match crate::api::parties::repayment_account(conn, user, method, None).await {
        Ok(Some(id)) => (
            account_by_id(conn, id).await?,
            crate::api::parties::repayment_fee(conn, id, method, supplier, sum).await?,
        ),
        Ok(None) => return Ok(Err(crate::api::receipts::NO_OUTSIDE.into())),
        Err(e) => return Ok(Err(reason(e)?)),
    };
    let method_word = match method {
        "cash" => "наличные",
        "card" => "карта",
        _ => "перевод",
    };
    let fee_line = if fee != 0 {
        format!(
            "\nКомиссия банка: {} (на счёт придёт {})",
            format_som(fee),
            format_som(sum.saturating_sub(fee))
        )
    } else {
        String::new()
    };
    let summary = format!(
        "{}\n{}: {} (сейчас {})\nСумма: {}\nЧем: {}\n{}: {}{fee_line}\nКомментарий: {}",
        if supplier {
            "Оплата поставщику"
        } else {
            "Погашение долга клиентом"
        },
        if supplier {
            "Поставщик"
        } else {
            "Клиент"
        },
        one_line(&p.name),
        balance_words(p.balance_tyiyn),
        format_som(sum),
        method_word,
        if supplier { "Откуда" } else { "Куда" },
        src.line(),
        quoted(&text)
    );
    Ok(Ok(Built {
        op: DraftOp::Repayment(RepaymentReq {
            op_id,
            party_id: p.id,
            amount_tyiyn: sum,
            method: Some(method.into()),
            comment: text,
            receipt_id: None,
            account_id: None,
        }),
        kind: if supplier {
            "supplier_payment"
        } else {
            "repayment"
        },
        summary,
        hint: if supplier {
            balance_hint(&src)
        } else {
            String::new()
        },
    }))
}

async fn staff(conn: &mut PgConnection, user: &CurrentUser, op_id: Uuid, input: &Value) -> Step {
    let branch_id = user.branch_id;
    let operation = norm(&take!(need(input, "operation", "что сделать")));
    let who = take!(need(input, "employee", "сотрудник"));
    let e = take!(employee(conn, branch_id, &who).await?);
    let text = comment(input, "comment");
    if operation.starts_with("прем")
        || operation.starts_with("бонус")
        || operation.starts_with("удерж")
        || operation.starts_with("штраф")
    {
        let bonus = operation.starts_with("прем") || operation.starts_with("бонус");
        let sum = take!(amount(input, "amount"));
        let summary = format!(
            "{}\nСотрудник: {}\nСумма: {}\nПричина: {}",
            if bonus {
                "Премия"
            } else {
                "Удержание"
            },
            one_line(&e.name),
            format_som(sum),
            quoted(&text)
        );
        return Ok(Ok(Built {
            op: DraftOp::Accrual(AccrualReq {
                op_id,
                employee_id: e.id,
                kind: if bonus { "bonus" } else { "penalty" }.into(),
                amount_tyiyn: sum,
                business_date: None,
                comment: text,
            }),
            kind: if bonus { "bonus" } else { "penalty" },
            summary,
            hint: String::new(),
        }));
    }
    if operation.starts_with("выпл")
        || operation.starts_with("выда")
        || operation.starts_with("аванс")
    {
        let advance = operation.starts_with("аванс");
        let sum = take!(amount(input, "amount"));
        let src_q = take!(field(input, "source"));
        let src = take!(account(conn, branch_id, src_q.as_deref()).await?);
        let earned = sqlx::query_scalar!(
            r#"select coalesce((select sum(amount_tyiyn) from payroll_accruals where employee_id = $1), 0)::bigint
                    - coalesce((select sum(amount_tyiyn) from payouts where employee_id = $1), 0)::bigint as "b!""#,
            e.id
        )
        .fetch_one(&mut *conn)
        .await?;
        let (source, account_id) = match &src {
            Source::Account { id, .. } => ("account", Some(*id)),
        };
        let summary = format!(
            "{}\nСотрудник: {} (к выплате сейчас {})\nСумма: {}\nОткуда: {}\nКомментарий: {}",
            if advance {
                "Аванс сотруднику"
            } else {
                "Выплата сотруднику"
            },
            one_line(&e.name),
            format_som(earned),
            format_som(sum),
            src.line(),
            quoted(&text)
        );
        let hint = if !advance && sum > earned {
            format!(
                "{} Если это аванс сверх заработанного — так и скажите.",
                balance_hint(&src)
            )
        } else {
            balance_hint(&src)
        };
        return Ok(Ok(Built {
            op: DraftOp::Payout(PayoutReq {
                op_id,
                employee_id: e.id,
                amount_tyiyn: sum,
                source: Some(source.into()),
                account_id,
                advance,
                comment: text,
            }),
            kind: if advance { "advance" } else { "payout" },
            summary,
            hint,
        }));
    }
    if operation.starts_with("оклад") || operation.starts_with("зарплат") {
        let today = today(conn).await?;
        let month = match take!(field(input, "month")) {
            None => today.with_day(1).unwrap_or(today),
            Some(s) => take!(
                NaiveDate::parse_from_str(&format!("{}-01", s.get(..7).unwrap_or(&s)), "%Y-%m-%d")
                    .map_err(|_| format!("месяц «{s}» не разобрать: нужен ГГГГ-ММ"))
            ),
        };
        let typed = match take!(field(input, "amount")) {
            None => None,
            Some(_) => Some(take!(amount(input, "amount"))),
        };
        // Сумма по ставке — та же, что возьмёт начисление: на последний день месяца,
        // не позже сегодня.
        let last = month
            .checked_add_months(chrono::Months::new(1))
            .and_then(|d| d.pred_opt())
            .unwrap_or(month)
            .min(today);
        let rule = crate::api::payroll::active_rule(conn, branch_id, e.id, "monthly_salary", last)
            .await?
            .and_then(|r| r.1);
        // Сумма по ставке пишется в черновик: проводится ровно показанная, даже если ставку
        // успели поменять до нажатия (ADR-070).
        let (amount, shown) = match (typed, rule) {
            (Some(v), _) => (v, format!("{} (указана)", format_som(v))),
            (None, Some(v)) => (v, format!("{} (по ставке)", format_som(v))),
            (None, None) => {
                return Ok(Err(format!(
                    "у {} нет ставки оклада — назовите сумму",
                    e.name
                )));
            }
        };
        let summary = format!(
            "Оклад за месяц\nСотрудник: {}\nМесяц: {}\nСумма: {shown}",
            one_line(&e.name),
            month_name(month)
        );
        return Ok(Ok(Built {
            op: DraftOp::Salary(SalaryReq {
                op_id,
                employee_id: e.id,
                month,
                amount_tyiyn: Some(amount),
            }),
            kind: "salary",
            summary,
            hint: String::new(),
        }));
    }
    Ok(Err(
        "операция с сотрудником: «премия», «удержание», «выплата», «аванс» или «оклад»".into(),
    ))
}

async fn goods(conn: &mut PgConnection, user: &CurrentUser, op_id: Uuid, input: &Value) -> Step {
    let operation = norm(&take!(need(input, "operation", "что сделать")));
    let q = take!(need(input, "product", "товар"));
    let value = take!(need(input, "value", "значение"));
    let p = take!(product(conn, user, &q).await?);
    let name = one_line(&p.name);
    if operation.contains("миним") {
        let qty = match p.unit.as_str() {
            "ml" | "g" => take!(
                parse_liters(value.trim_end_matches(|c: char| c.is_alphabetic()
                    || c == '.'
                    || c.is_whitespace()))
                .ok_or_else(|| format!("количество «{value}» не разобрать"))
            ),
            _ => take!(
                value
                    .trim_end_matches(|c: char| c.is_alphabetic() || c == '.' || c.is_whitespace())
                    .replace(char::is_whitespace, "")
                    .parse::<i64>()
                    .ok()
                    .filter(|v| *v >= 0)
                    .ok_or_else(|| format!(
                        "количество «{value}» не разобрать: нужно целое число штук"
                    ))
            ),
        };
        let summary = format!(
            "Минимальный остаток\nТовар: {name}\nСейчас: {}\nНовый: {}",
            qty_text(p.min_stock, &p.unit),
            qty_text(qty, &p.unit)
        );
        return Ok(Ok(Built {
            op: DraftOp::Prices {
                product_id: p.id,
                req: PricesReq {
                    sale_price_tyiyn: None,
                    pour_price_per_l_tyiyn: None,
                    min_stock: Some(qty),
                },
            },
            kind: "min_stock",
            summary,
            hint: String::new(),
        }));
    }
    let price = take!(
        parse_spoken_som(&value)
            .filter(|v| *v > 0)
            .ok_or_else(|| format!("цену «{value}» не разобрать: нужны цифры, например «350,50»"))
    );
    if operation.contains("закуп") {
        let summary = format!(
            "Цена закупки (товар продан без прихода)\nТовар: {name}\nЦена: {} {}",
            format_som(price),
            per_text(&p)
        );
        return Ok(Ok(Built {
            op: DraftOp::PurchasePrice {
                product_id: p.id,
                req: PurchasePriceReq {
                    op_id,
                    price_tyiyn: price,
                },
            },
            kind: "purchase_price",
            summary,
            hint: String::new(),
        }));
    }
    if operation.contains("продаж") || operation == "цена" {
        let mut summary = format!(
            "Цена продажи\nТовар: {name}\nСейчас: {} {per}\nНовая: {} {per}",
            format_som(p.sale_price_tyiyn),
            format_som(price),
            per = per_text(&p)
        );
        // У масла в канистре розлив за литр отдельно не задаётся — он идёт от цены канистры (ADR-074).
        if let Some(pour) = crate::domain::money::pour_price_per_l(&p.unit, p.container_ml, price) {
            summary.push_str(&format!("\nРозлив за литр станет: {}", format_som(pour)));
        }
        return Ok(Ok(Built {
            op: DraftOp::Prices {
                product_id: p.id,
                req: PricesReq {
                    sale_price_tyiyn: Some(price),
                    pour_price_per_l_tyiyn: None,
                    min_stock: None,
                },
            },
            kind: "sale_price",
            summary,
            hint: String::new(),
        }));
    }
    Ok(Err(
        "операция с товаром: «цена продажи», «цена закупки» или «минимальный остаток»".into(),
    ))
}

async fn receipt(conn: &mut PgConnection, user: &CurrentUser, op_id: Uuid, input: &Value) -> Step {
    let branch_id = user.branch_id;
    let supplier = match take!(field(input, "supplier")) {
        None => None,
        Some(q) => Some(take!(
            find_party(conn, branch_id, Some("supplier"), &q).await?
        )),
    };
    let on_debt = take!(field(input, "payment")).is_some_and(|p| norm(&p).contains("долг"));
    if on_debt && supplier.is_none() {
        return Ok(Err("для прихода в долг назовите поставщика".into()));
    }
    let src = if on_debt {
        None
    } else {
        let q = take!(field(input, "paid_from"));
        let Some(q) = q else {
            return Ok(Err(
                "откуда оплачен приход: «Касса», «Счёт», сейф — или «в долг»".into(),
            ));
        };
        Some(take!(account(conn, branch_id, Some(&q)).await?))
    };
    let Some(lines_in) = input.get("lines").and_then(Value::as_array) else {
        return Ok(Err("нет строк прихода (поле lines)".into()));
    };
    if lines_in.is_empty() {
        return Ok(Err("нет строк прихода".into()));
    }
    if lines_in.len() > MAX_LINES {
        return Ok(Err(format!(
            "в приходе через бот не больше {MAX_LINES} строк — большой приход внесите в программе"
        )));
    }
    let mut lines = Vec::with_capacity(lines_in.len());
    let mut shown = Vec::with_capacity(lines_in.len());
    let mut total: i64 = 0;
    for (i, l) in lines_in.iter().enumerate() {
        let n = i + 1;
        let q = take!(need(l, "product", "товар").map_err(|e| format!("строка {n}: {e}")));
        let qty_s =
            take!(need(l, "quantity", "количество").map_err(|e| format!("строка {n}: {e}")));
        let p = take!(
            product(conn, user, &q)
                .await?
                .map_err(|e| format!("Строка {n}. {e}"))
        );
        let count = take!(receipt_count(&qty_s).ok_or_else(|| format!(
            "строка {n}: количество «{qty_s}» не понять — напишите целым числом штук или канистр, например «10» или «10 шт»"
        )));
        let price = take!(amount(l, "price").map_err(|e| format!("строка {n}: {e}")));
        // Сумму строки считает сервер: количество × цена за единицу (инвариант 1).
        let (qty, unit_text) = match (p.unit.as_str(), p.container_ml) {
            ("piece", _) => (count, format!("{count} шт")),
            ("ml", Some(ml)) if ml > 0 => (
                take!(
                    count
                        .checked_mul(ml)
                        .ok_or_else(|| "слишком большое количество".to_string())
                ),
                format!("{count} кан. по {}", qty_text(ml, "ml")),
            ),
            _ => {
                return Ok(Err(format!(
                    "строка {n}: «{}» на розлив или на вес — такой приход внесите в программе",
                    p.name
                )));
            }
        };
        let sum = take!(
            count
                .checked_mul(price)
                .filter(|v| ops::check_amount(*v).is_ok())
                .ok_or_else(|| format!("строка {n}: слишком большая сумма"))
        );
        total = take!(
            total
                .checked_add(sum)
                .ok_or_else(|| "слишком большая сумма".to_string())
        );
        shown.push(format!(
            "{n}. {} — {unit_text} × {} = {}",
            one_line(&p.name),
            format_som(price),
            format_som(sum)
        ));
        lines.push(ReceiptLineReq {
            product_id: p.id,
            qty,
            cost_tyiyn: sum,
            sale_price_tyiyn: None,
        });
    }
    let doc = comment(input, "supplier_doc");
    let text = comment(input, "comment");
    let pay_line = match &src {
        None => "в долг поставщику".to_string(),
        Some(s) => format!("оплачено из «{}»", s.line()),
    };
    let summary = format!(
        "Приход товара\nПоставщик: {}\n{}\nИтого: {}\nРасчёт: {pay_line}\nДокумент поставщика: {}\nКомментарий: {}",
        supplier
            .as_ref()
            .map_or_else(|| "без поставщика".to_string(), |s| one_line(&s.name)),
        shown.join("\n"),
        format_som(total),
        quoted(&doc),
        quoted(&text)
    );
    let paid_from = src
        .as_ref()
        .map(|Source::Account { id, .. }| id.to_string());
    let hint = src.as_ref().map(balance_hint).unwrap_or_default();
    Ok(Ok(Built {
        op: DraftOp::Receipt(ReceiptReq {
            op_id,
            supplier_id: supplier.map(|s| s.id),
            payment: Some(if on_debt { "debt" } else { "paid" }.into()),
            supplier_doc: doc,
            comment: text,
            lines,
            paid_from,
            paid_now_tyiyn: None,
        }),
        kind: "receipt",
        summary,
        hint,
    }))
}

// ---------- Кнопки под черновиком ----------

/// Нажатие «✅ Провести» (`d:ok:<id>`) или «✖ Отмена» (`d:no:<id>`) в чате `chat_id` пользователем
/// Телеграма `from_id`. Черновик принимается только от того же чата, того же владельца и филиала,
/// от того, кто привязал чат (ADR-070), открытый и не просроченный.
/// Ответ — текст для чата; `Err` — только сбой базы.
pub async fn press(
    conn: &mut PgConnection,
    chat_id: i64,
    from_id: i64,
    data: &str,
) -> AppResult<String> {
    let mut parts = data.splitn(3, ':');
    let (Some("d"), Some(verb), Some(id)) = (parts.next(), parts.next(), parts.next()) else {
        return Ok("Кнопка устарела.".into());
    };
    let (Some(confirm), Ok(id)) = (
        match verb {
            "ok" => Some(true),
            "no" => Some(false),
            _ => None,
        },
        Uuid::parse_str(id),
    ) else {
        return Ok("Кнопка устарела.".into());
    };
    let Some((owner, tg_user)) = linked_owner(conn, chat_id).await? else {
        return Ok(NOT_YOURS.into());
    };
    if tg_user != Some(from_id) {
        return Ok(
            "Провести или отменить черновик может только владелец, который подключил этот чат. Ничего не проведено."
                .into(),
        );
    }
    // Транзакция завершается явно в любом случае: брошенная держала бы блокировку черновика.
    let mut tx = conn.begin().await?;
    let reply = decide(&mut tx, owner, chat_id, id, confirm).await;
    match reply {
        Ok(text) => {
            tx.commit().await?;
            Ok(text)
        }
        Err(e) => {
            tx.rollback().await?;
            Err(e)
        }
    }
}

/// Судьба черновика по нажатию — внутри транзакции `press`.
async fn decide(
    conn: &mut PgConnection,
    owner: CurrentUser,
    chat_id: i64,
    id: Uuid,
    confirm: bool,
) -> AppResult<String> {
    let Some(d) = sqlx::query!(
        r#"select chat_id, user_id, branch_id, payload, status, result,
                  expires_at <= now() as "expired!"
           from bot_drafts where id = $1 for update"#,
        id
    )
    .fetch_optional(&mut *conn)
    .await?
    else {
        return Ok(NOT_YOURS.into());
    };
    if d.chat_id != chat_id || d.user_id != owner.id || d.branch_id != owner.branch_id {
        return Ok(NOT_YOURS.into());
    }
    let result_text = |r: &Option<Value>, key: &str| {
        r.as_ref()
            .and_then(|v| v.get(key))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    match d.status.as_str() {
        "open" => {}
        "posted" => {
            return Ok(format!(
                "Уже проведено: {}.",
                result_text(&d.result, "text")
            ));
        }
        "cancelled" => return Ok("Черновик уже снят — ничего не проведено.".into()),
        "expired" => return Ok("Черновик устарел — ничего не проведено.".into()),
        _ => {
            return Ok(format!(
                "Черновик уже отклонён: {}.",
                result_text(&d.result, "reason")
            ));
        }
    }
    if !confirm {
        finish(
            &mut *conn,
            id,
            "cancelled",
            json!({ "reason": "отменён владельцем" }),
        )
        .await?;
        return Ok("Черновик отменён — ничего не проведено.".into());
    }
    if d.expired {
        finish(
            &mut *conn,
            id,
            "expired",
            json!({ "reason": "прошло 10 минут" }),
        )
        .await?;
        return Ok(
            "Черновик устарел (прошло больше 10 минут) — ничего не проведено. Опишите операцию заново."
                .into(),
        );
    }
    if posted_today_locked(&mut *conn, owner.branch_id).await? >= DAILY_LIMIT {
        finish(&mut *conn, id, "rejected", json!({ "reason": LIMIT_TEXT })).await?;
        return Ok(LIMIT_TEXT.into());
    }
    let op: DraftOp =
        serde_json::from_value(d.payload).map_err(|e| AppError::Internal(e.to_string()))?;
    // За 10 минут сотрудника или клиента могли отключить, товар — убрать в архив: такой черновик
    // не проводится (ADR-070).
    if let Some(why) = stale_refs(&mut *conn, owner.branch_id, &op).await? {
        finish(&mut *conn, id, "rejected", json!({ "reason": why })).await?;
        return Ok(format!(
            "Не проведено: {why}. Черновик закрыт — проверьте данные и опишите операцию заново."
        ));
    }
    let ctx = Ctx::bot(owner, chat_id);
    // Проведение — во вложенной транзакции: отказ проверки откатывает только его, а судьба
    // черновика записывается.
    let mut sp = conn.begin().await?;
    match execute(&mut sp, &ctx, op).await {
        Ok(p) => {
            sp.commit().await?;
            finish(&mut *conn, id, "posted", json!(p)).await?;
            Ok(format!("Проведено: {}.", p.text))
        }
        Err(e) => {
            sp.rollback().await?;
            let why = reason(e)?;
            finish(&mut *conn, id, "rejected", json!({ "reason": why })).await?;
            Ok(format!(
                "Не проведено: {why}. Черновик закрыт — проверьте данные и опишите операцию заново."
            ))
        }
    }
}

/// Что из черновика перестало действовать: сотрудник или клиент отключён, товар в архиве.
async fn stale_refs(
    conn: &mut PgConnection,
    branch_id: Uuid,
    op: &DraftOp,
) -> AppResult<Option<String>> {
    let (employee, party, products): (Option<Uuid>, Option<Uuid>, Vec<Uuid>) = match op {
        DraftOp::Accrual(r) => (Some(r.employee_id), None, Vec::new()),
        DraftOp::Payout(r) => (Some(r.employee_id), None, Vec::new()),
        DraftOp::Salary(r) => (Some(r.employee_id), None, Vec::new()),
        DraftOp::Repayment(r) => (None, Some(r.party_id), Vec::new()),
        DraftOp::DebtAdjust(r) => (None, Some(r.party_id), Vec::new()),
        DraftOp::Prices { product_id, .. } | DraftOp::PurchasePrice { product_id, .. } => {
            (None, None, vec![*product_id])
        }
        DraftOp::Receipt(r) => (
            None,
            r.supplier_id,
            r.lines.iter().map(|l| l.product_id).collect(),
        ),
        DraftOp::Expense(_) | DraftOp::CashMovement(_) | DraftOp::CashTransfer(_) => {
            (None, None, Vec::new())
        }
    };
    if let Some(id) = employee {
        let e = sqlx::query!(
            "select full_name, active from employees where id = $1 and branch_id = $2",
            id,
            branch_id
        )
        .fetch_optional(&mut *conn)
        .await?;
        match e {
            Some(e) if e.active => {}
            Some(e) => {
                return Ok(Some(format!(
                    "сотрудник «{}» отключён",
                    one_line(&e.full_name)
                )));
            }
            None => return Ok(Some("сотрудник не найден".into())),
        }
    }
    if let Some(id) = party {
        let p = sqlx::query!(
            "select name, active from parties where id = $1 and branch_id = $2",
            id,
            branch_id
        )
        .fetch_optional(&mut *conn)
        .await?;
        match p {
            Some(p) if p.active => {}
            Some(p) => return Ok(Some(format!("«{}» отключён", one_line(&p.name)))),
            None => return Ok(Some("клиент или поставщик не найден".into())),
        }
    }
    if !products.is_empty() {
        let archived = sqlx::query_scalar!(
            "select name from products where id = any($1) and archived order by name",
            &products
        )
        .fetch_all(&mut *conn)
        .await?;
        if let Some(name) = archived.first() {
            return Ok(Some(format!("товар «{}» убран в архив", one_line(name))));
        }
    }
    Ok(None)
}

async fn finish(conn: &mut PgConnection, id: Uuid, status: &str, result: Value) -> AppResult<()> {
    sqlx::query!(
        "update bot_drafts set status = $2, result = $3, finished_at = now() where id = $1 and status = 'open'",
        id,
        status,
        result
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_match_by_word_starts() {
        assert!(sounds_like("Касса", "из кассы"));
        assert!(sounds_like("Счёт", "со счета"));
        assert!(sounds_like("Сейф владельца", "сейф владельца"));
        assert!(sounds_like("Сейф магазина", "сейф"));
        assert!(sounds_like("Хозтовары", "на хозтовары"));
        assert!(sounds_like("Бакыт Асанов", "Бакыту"));
        assert!(!sounds_like("Налоги", "хозтовары"));
        assert!(!sounds_like("Касса", "из"));
    }

    #[test]
    fn choose_asks_when_many() {
        fn pick(v: Vec<&'static str>, q: &str) -> Result<&'static str, String> {
            choose(
                v,
                |s| s.to_string(),
                |s| norm(s) == norm(q),
                "нет".into(),
                "несколько".into(),
            )
        }
        assert_eq!(pick(vec!["Азамат"], "азамат"), Ok("Азамат"));
        assert_eq!(pick(vec!["Азамат", "Азамат Б."], "азамат"), Ok("Азамат"));
        let many = pick(vec!["Азамат А.", "Азамат Б."], "азамат");
        assert!(
            many.as_ref()
                .is_err_and(|t| t.contains("Азамат А.") && t.contains("уточните"))
        );
        assert_eq!(pick(vec![], "x"), Err("нет".into()));
    }

    #[test]
    fn draft_payload_round_trips() {
        let op = DraftOp::Prices {
            product_id: Uuid::nil(),
            req: PricesReq {
                sale_price_tyiyn: Some(55_000),
                pour_price_per_l_tyiyn: None,
                min_stock: None,
            },
        };
        let v = serde_json::to_value(&op).unwrap_or_default();
        assert_eq!(v["op"], "prices");
        assert!(matches!(
            serde_json::from_value::<DraftOp>(v),
            Ok(DraftOp::Prices { .. })
        ));
        assert_eq!(qty_text(2500, "ml"), "2,5 л");
        assert_eq!(qty_text(4000, "ml"), "4 л");
        assert_eq!(qty_text(3, "piece"), "3 шт");
    }
}
