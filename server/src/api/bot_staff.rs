//! Кабинет работника в Телеграм-боте (SPEC-23, ADR-066): заработок за день и за месяц,
//! удержания и премии, свои работы. Только просмотр своего: каждая выборка ограничена
//! `employee_id` и филиалом чата, сотрудник из данных кнопки сверяется с привязкой чата.
//! Суммы — из тех же начислений, что экран «Расчёт» (`api::payroll`), со сторно. Выплаты и
//! остаток к выплате здесь не показываются (решение заказчика).

use chrono::{Datelike, Months, NaiveDate};
use serde_json::json;
use sqlx::{Connection, PgConnection};
use uuid::Uuid;

use crate::api::payroll::{self, PayItem, StaffPay};
use crate::api::telegram::{Button, Keys, Reply, bot_journal, command, phone_key};
use crate::auth::Ctx;
use crate::domain::money::{format_som, format_som_signed};
use crate::error::AppResult;
use crate::ops;

/// Действий в минуту на чат работника (сообщения и кнопки).
pub const RATE_PER_MIN: i64 = 30;
/// Строк в «Мои работы»: замен у мастера и смен у кассира.
pub const WORKS: i64 = 30;
/// На сколько месяцев назад листается «Месяц».
const MONTHS_BACK: u32 = 12;

// Кнопки внизу чата работника; «Мой заработок» — в чате, где номер ещё и у клиента.
pub const BTN_DAY: &str = "💼 Сегодня";
pub const BTN_MONTH: &str = "📅 Месяц";
pub const BTN_ADJUST: &str = "🧾 Удержания и премии";
pub const BTN_WORKS: &str = "🔧 Мои работы";
pub const BTN_EARNINGS: &str = "💼 Мой заработок";
pub const BUTTONS: [&str; 5] = [BTN_DAY, BTN_MONTH, BTN_ADJUST, BTN_WORKS, BTN_EARNINGS];

/// Чат работника отвязан проверкой: карточка отключена или телефон в ней другой.
pub const RESHARE: &str = "Номер в карточке изменился — поделитесь номером снова: нажмите «📱 Поделиться номером». Если вашу карточку отключили, обратитесь к владельцу.";
/// Номер записан у нескольких активных сотрудников: не угадываем.
pub const AMBIGUOUS: &str = "Этот номер записан у нескольких сотрудников, и мы не знаем, кто из них вы. Обратитесь к владельцу — номер оставят в одной карточке, и подключитесь снова.";
pub const HELP: &str = "Нажмите кнопку внизу:\n💼 Сегодня — сколько заработали за день\n📅 Месяц — за месяц и по дням\n🧾 Удержания и премии — с комментарием владельца\n🔧 Мои работы — ваши замены или смены";
const TOO_OFTEN: &str = "Слишком часто — подождите минуту.";
const STALE: &str = "Кнопка устарела — нажмите кнопку внизу.";
const STALE_REASON: &str = "карточка сотрудника отключена или телефон в ней изменён";

const MONTHS: [&str; 12] = [
    "Январь",
    "Февраль",
    "Март",
    "Апрель",
    "Май",
    "Июнь",
    "Июль",
    "Август",
    "Сентябрь",
    "Октябрь",
    "Ноябрь",
    "Декабрь",
];

/// Сотрудник, к которому привязан чат.
pub struct Employee {
    pub id: Uuid,
    pub branch_id: Uuid,
    pub name: String,
    pub is_cashier: bool,
    pub is_master: bool,
}

/// Чат работника: подключён, не подключён или только что отвязан проверкой.
pub enum EmployeeLink {
    None,
    Linked(Employee),
    Dropped,
}

/// Подключение действует, пока сотрудник активен, в том же филиале и телефон в карточке тот
/// же, каким он подключился. Иначе чат отвязывается с записью в журнал.
pub async fn employee_link(conn: &mut PgConnection, chat_id: i64) -> AppResult<EmployeeLink> {
    let r = sqlx::query!(
        r#"select t.employee_id, t.branch_id as link_branch, t.phone as link_phone,
                  e.branch_id, e.full_name, e.phone, e.active, e.is_cashier, e.is_master
           from telegram_employees t join employees e on e.id = t.employee_id
           where t.chat_id = $1"#,
        chat_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    let Some(r) = r else {
        return Ok(EmployeeLink::None);
    };
    let card = phone_key(&r.phone);
    if r.active
        && r.branch_id == r.link_branch
        && card.is_some()
        && card == phone_key(&r.link_phone)
    {
        return Ok(EmployeeLink::Linked(Employee {
            id: r.employee_id,
            branch_id: r.branch_id,
            name: r.full_name,
            is_cashier: r.is_cashier,
            is_master: r.is_master,
        }));
    }
    drop_chat(conn, chat_id, r.employee_id, r.link_branch, STALE_REASON).await?;
    Ok(EmployeeLink::Dropped)
}

/// Отвязать чат работника — одной транзакцией с записью в журнал.
pub async fn drop_chat(
    conn: &mut PgConnection,
    chat_id: i64,
    employee_id: Uuid,
    branch_id: Uuid,
    reason: &str,
) -> AppResult<()> {
    let mut tx = conn.begin().await?;
    let gone = sqlx::query!(
        "delete from telegram_employees where chat_id = $1 and employee_id = $2",
        chat_id,
        employee_id
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if gone > 0 {
        bot_journal(
            &mut tx,
            branch_id,
            "telegram.employee_unlink",
            "employee",
            Some(employee_id),
            json!({ "chat": chat_id, "reason": reason }),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Карточку сотрудника отключили или сменили в ней телефон: его чат отвязывается сразу, в той же
/// транзакции, что и правка карточки.
pub(crate) async fn drop_for_employee(
    conn: &mut PgConnection,
    ctx: &Ctx,
    employee_id: Uuid,
    reason: &str,
) -> AppResult<()> {
    let chats = sqlx::query_scalar!(
        "delete from telegram_employees where employee_id = $1 and branch_id = $2 returning chat_id",
        employee_id,
        ctx.user.branch_id
    )
    .fetch_all(&mut *conn)
    .await?;
    for chat in chats {
        ops::audit(
            conn,
            ctx,
            "telegram.employee_unlink",
            "employee",
            Some(employee_id),
            json!({ "chat": chat, "reason": reason }),
        )
        .await?;
    }
    Ok(())
}

/// Активные сотрудники с этим номером (полный номер, ADR-090).
pub async fn by_phone(conn: &mut PgConnection, key: &str) -> AppResult<Vec<Employee>> {
    Ok(sqlx::query!(
        r#"select id, branch_id, full_name, is_cashier, is_master from employees
           where active and phone_norm(phone) = $1
           order by created_at"#,
        key
    )
    .fetch_all(&mut *conn)
    .await?
    .into_iter()
    .map(|r| Employee {
        id: r.id,
        branch_id: r.branch_id,
        name: r.full_name,
        is_cashier: r.is_cashier,
        is_master: r.is_master,
    })
    .collect())
}

/// Привязать чат к сотруднику: у сотрудника один чат, у чата один сотрудник. Владельцу —
/// уведомление `telegram.employee_link`, если чат подключился к нему впервые.
pub async fn link(
    conn: &mut PgConnection,
    chat_id: i64,
    e: &Employee,
    phone: &str,
) -> AppResult<()> {
    let mut tx = conn.begin().await?;
    let was = sqlx::query!(
        "select employee_id, branch_id from telegram_employees where chat_id = $1 for update",
        chat_id
    )
    .fetch_optional(&mut *tx)
    .await?;
    let old_chat = sqlx::query_scalar!(
        "select chat_id from telegram_employees where employee_id = $1 and chat_id <> $2 for update",
        e.id,
        chat_id
    )
    .fetch_optional(&mut *tx)
    .await?;
    sqlx::query!(
        "delete from telegram_employees where chat_id = $1 or employee_id = $2",
        chat_id,
        e.id
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query!(
        r#"insert into telegram_employees (chat_id, employee_id, branch_id, phone)
           values ($1, $2, $3, $4)"#,
        chat_id,
        e.id,
        e.branch_id,
        phone
    )
    .execute(&mut *tx)
    .await?;
    if let Some(w) = &was
        && w.employee_id != e.id
    {
        bot_journal(
            &mut tx,
            w.branch_id,
            "telegram.employee_unlink",
            "employee",
            Some(w.employee_id),
            json!({ "chat": chat_id, "reason": "чат подключился к другому сотруднику" }),
        )
        .await?;
    }
    if was.as_ref().is_none_or(|w| w.employee_id != e.id) {
        bot_journal(
            &mut tx,
            e.branch_id,
            "telegram.employee_link",
            "employee",
            Some(e.id),
            json!({ "chat": chat_id, "name": e.name, "replaced_chat": old_chat }),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Не больше 30 действий в минуту на чат. Просмотры в журнал не пишутся — счёт в `telegram_hits`.
pub async fn too_often(conn: &mut PgConnection, chat_id: i64) -> AppResult<bool> {
    sqlx::query!("delete from telegram_hits where at < now() - interval '1 minute'")
        .execute(&mut *conn)
        .await?;
    sqlx::query!("insert into telegram_hits (chat_id) values ($1)", chat_id)
        .execute(&mut *conn)
        .await?;
    let n = sqlx::query_scalar!(
        r#"select count(*) as "n!" from telegram_hits
           where chat_id = $1 and at > now() - interval '1 minute'"#,
        chat_id
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(n > RATE_PER_MIN)
}

pub fn too_often_reply() -> Reply {
    Reply::new(TOO_OFTEN, Keys::Employee)
}

/// Раздел кабинета.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Day,
    Month,
    Adjust,
    Works,
}

/// Раздел по кнопке или команде; иначе `None` — это не раздел кабинета.
pub fn section(text: &str) -> Option<Section> {
    let t = text.trim();
    match t {
        BTN_DAY | BTN_EARNINGS => return Some(Section::Day),
        BTN_MONTH => return Some(Section::Month),
        BTN_ADJUST => return Some(Section::Adjust),
        BTN_WORKS => return Some(Section::Works),
        _ => {}
    }
    // Команды словом: «/месяц», «сегодня». «Мои …» — у клиента «Мои машины», поэтому работы —
    // только кнопкой или «/работы».
    match command(t).as_str() {
        "сегодня" | "заработок" => Some(Section::Day),
        "месяц" => Some(Section::Month),
        "удержания" | "премии" => Some(Section::Adjust),
        "работы" => Some(Section::Works),
        _ => None,
    }
}

/// Ответ раздела. `both` — номер ещё и у клиента: внизу чата клиентские кнопки, поэтому
/// разделы кабинета даются кнопками под сообщением.
pub async fn show(
    conn: &mut PgConnection,
    e: &Employee,
    s: Section,
    both: bool,
) -> AppResult<Reply> {
    let text = match s {
        Section::Day => day_text(conn, e).await?,
        Section::Month => return month(conn, e, None, both).await,
        Section::Adjust => adjust_text(conn, e).await?,
        Section::Works => works_text(conn, e).await?,
    };
    Ok(reply(e, text, both, Vec::new()))
}

/// Подсказка на свободный текст: вопросы обычным текстом работнику недоступны (SPEC-23).
pub fn help(e: &Employee, both: bool) -> Reply {
    reply(e, HELP.to_string(), both, Vec::new())
}

fn button(text: impl Into<String>, data: String) -> Button {
    Button {
        text: text.into(),
        data,
    }
}

/// Кнопки разделов под сообщением; в данных — сотрудник, сервер сверяет его с привязкой чата.
fn sections_rows(e: &Employee) -> Vec<Vec<Button>> {
    vec![
        vec![
            button(BTN_DAY, format!("e:t:{}", e.id)),
            button(BTN_MONTH, format!("e:m:{}", e.id)),
        ],
        vec![
            button(BTN_ADJUST, format!("e:a:{}", e.id)),
            button(BTN_WORKS, format!("e:w:{}", e.id)),
        ],
    ]
}

fn reply(e: &Employee, text: String, both: bool, mut rows: Vec<Vec<Button>>) -> Reply {
    if both {
        rows.extend(sections_rows(e));
    }
    Reply {
        text,
        keys: Keys::Employee,
        inline: rows,
        follow: None,
    }
}

/// Нажатие кнопки под сообщением: `e:<раздел>:<сотрудник>[:ГГГГ-ММ]`. Чужой сотрудник,
/// непонятный раздел или месяц вне пределов — ничего не показывается.
pub async fn callback(
    conn: &mut PgConnection,
    e: &Employee,
    data: &str,
    both: bool,
) -> AppResult<Reply> {
    let mut parts = data.split(':');
    let (Some("e"), Some(verb), Some(id)) = (parts.next(), parts.next(), parts.next()) else {
        return Ok(Reply::new(STALE, Keys::Employee));
    };
    if Uuid::parse_str(id).ok() != Some(e.id) {
        return Ok(Reply::new(STALE, Keys::Employee));
    }
    let arg = parts.next();
    if parts.next().is_some() {
        return Ok(Reply::new(STALE, Keys::Employee));
    }
    match (verb, arg) {
        ("t", None) => show(conn, e, Section::Day, both).await,
        ("a", None) => show(conn, e, Section::Adjust, both).await,
        ("w", None) => show(conn, e, Section::Works, both).await,
        ("m", None) => month(conn, e, None, both).await,
        ("m", Some(m)) => match parse_month(m) {
            Some(m) => month(conn, e, Some(m), both).await,
            None => Ok(Reply::new(STALE, Keys::Employee)),
        },
        _ => Ok(Reply::new(STALE, Keys::Employee)),
    }
}

fn parse_month(s: &str) -> Option<NaiveDate> {
    if s.len() != 7 {
        return None;
    }
    NaiveDate::parse_from_str(&format!("{s}-01"), "%Y-%m-%d").ok()
}

fn first_of(d: NaiveDate) -> NaiveDate {
    d.with_day(1).unwrap_or(d)
}

fn month_name(d: NaiveDate) -> &'static str {
    MONTHS.get(d.month0() as usize).copied().unwrap_or("")
}

async fn today(conn: &mut PgConnection) -> AppResult<NaiveDate> {
    Ok(
        sqlx::query_scalar!(r#"select (now() at time zone 'Asia/Bishkek')::date as "d!""#)
            .fetch_one(&mut *conn)
            .await?,
    )
}

/// Порядок видов в тексте: сначала заработанное, потом премии и удержания.
const KIND_ORDER: [&str; 7] = [
    "revenue_percent",
    "service_fee",
    "shift_fee",
    "monthly_salary",
    "bonus",
    "penalty",
    "shortage",
];

/// Начисления сотрудника за дни учёта — та же выборка, что «кто сколько заработал» владельца.
async fn mine(
    conn: &mut PgConnection,
    e: &Employee,
    from: NaiveDate,
    to: NaiveDate,
) -> AppResult<Option<StaffPay>> {
    let mut pay = payroll::staff_pay(conn, e.branch_id, from, to)
        .await?
        .into_iter()
        .find(|p| p.employee_id == e.id);
    if let Some(p) = &mut pay {
        p.items.sort_by_key(|i| {
            KIND_ORDER
                .iter()
                .position(|k| *k == i.kind)
                .unwrap_or(KIND_ORDER.len())
        });
    }
    Ok(pay)
}

/// Вид начисления словами для работника: «% с чеков», «за замены: 5 × 30 с = 150 с».
fn item_line(i: &PayItem) -> String {
    let sum = format_som(i.amount_tyiyn);
    match i.kind.as_str() {
        "revenue_percent" => format!("% с чеков: {sum}"),
        "service_fee" => {
            if i.count > 0 && i.amount_tyiyn % i.count == 0 {
                format!(
                    "за замены: {} × {} = {sum}",
                    i.count,
                    format_som(i.amount_tyiyn / i.count)
                )
            } else {
                format!("за замены ({}): {sum}", i.count)
            }
        }
        "shift_fee" => format!("за смену: {sum}"),
        "monthly_salary" => format!("оклад: {sum}"),
        "bonus" => format!("премии: {sum}"),
        "penalty" => format!("удержания: {sum}"),
        "shortage" => format!("удержание недостачи: {sum}"),
        other => format!("{other}: {sum}"),
    }
}

async fn day_text(conn: &mut PgConnection, e: &Employee) -> AppResult<String> {
    let day = today(conn).await?;
    let mut lines = vec![format!(
        "💼 Сегодня, {} — {}",
        day.format("%d.%m.%Y"),
        e.name
    )];
    match mine(conn, e, day, day).await? {
        None => lines.push("За сегодня пока ничего не начислено.".into()),
        Some(p) => {
            lines.extend(p.items.iter().map(|i| format!("  {}", item_line(i))));
            lines.push(format!("Итого за день: {}", format_som(p.total_tyiyn)));
        }
    }
    Ok(lines.join("\n"))
}

/// Месяц: итог и виды начислений, строка на каждый день с начислениями, листание по месяцам —
/// не дальше текущего и не раньше 12 месяцев назад.
async fn month(
    conn: &mut PgConnection,
    e: &Employee,
    month: Option<NaiveDate>,
    both: bool,
) -> AppResult<Reply> {
    let current = first_of(today(conn).await?);
    let earliest = current
        .checked_sub_months(Months::new(MONTHS_BACK))
        .unwrap_or(current);
    let m = month.map_or(current, first_of);
    if m > current || m < earliest {
        return Ok(Reply::new(STALE, Keys::Employee));
    }
    // Итог месяца — строка экрана «Расчёт» за этот месяц; виды и дни — те же начисления.
    let data = payroll::month_data(conn, e.branch_id, Some(m)).await?;
    let row = data.rows.iter().find(|r| r.employee_id == e.id);
    let pay = mine(conn, e, data.from, data.to).await?;
    let days = payroll::employee_days(conn, e.branch_id, e.id, data.from, data.to).await?;
    let mut lines = vec![format!("📅 {} {} — {}", month_name(m), m.year(), e.name)];
    let total = row.map_or(0, |r| r.accrued_tyiyn);
    lines.push(format!("Итого за месяц: {}", format_som(total)));
    if let Some(p) = &pay {
        lines.extend(p.items.iter().map(|i| format!("  {}", item_line(i))));
    }
    if let Some(r) = row
        && r.salary_tyiyn == 0
        && r.salary_rule_tyiyn.is_some()
    {
        lines.push("Оклад за этот месяц ещё не начислен — его начисляют в конце месяца.".into());
    }
    if days.is_empty() {
        lines.push(String::new());
        lines.push("Начислений за этот месяц нет.".into());
    } else {
        lines.push(String::new());
        lines.push("По дням:".into());
        lines.extend(
            days.iter()
                .map(|(d, v)| format!("  {} — {}", d.format("%d.%m"), format_som(*v))),
        );
    }
    let mut nav = Vec::new();
    if let Some(prev) = m.checked_sub_months(Months::new(1))
        && prev >= earliest
    {
        nav.push(button(
            format!("← {}", month_name(prev)),
            format!("e:m:{}:{}", e.id, prev.format("%Y-%m")),
        ));
    }
    if let Some(next) = m.checked_add_months(Months::new(1))
        && next <= current
    {
        nav.push(button(
            format!("{} →", month_name(next)),
            format!("e:m:{}:{}", e.id, next.format("%Y-%m")),
        ));
    }
    let rows = if nav.is_empty() {
        Vec::new()
    } else {
        vec![nav]
    };
    Ok(reply(e, lines.join("\n"), both, rows))
}

/// Премии и удержания за текущий и прошлый месяц: дата, сумма, комментарий владельца; отменённые
/// сторно помечены «отменено» и в заработок не входят.
async fn adjust_text(conn: &mut PgConnection, e: &Employee) -> AppResult<String> {
    let current = first_of(today(conn).await?);
    let prev = current
        .checked_sub_months(Months::new(1))
        .unwrap_or(current);
    let to = current
        .checked_add_months(Months::new(1))
        .and_then(|d| d.pred_opt())
        .unwrap_or(current);
    let rows = payroll::accrual_rows(conn, e.branch_id, e.id, prev, to).await?;
    let mut lines = vec![format!("🧾 Удержания и премии — {}", e.name)];
    for m in [current, prev] {
        lines.push(String::new());
        lines.push(format!("{} {}:", month_name(m), m.year()));
        let list: Vec<_> = rows
            .iter()
            .filter(|r| {
                first_of(r.business_date) == m
                    && r.reversal_of.is_none()
                    && matches!(r.kind.as_str(), "bonus" | "penalty" | "shortage")
            })
            .collect();
        if list.is_empty() {
            lines.push("  нет".into());
            continue;
        }
        for r in list {
            let what = match r.kind.as_str() {
                "bonus" => "премия",
                "penalty" => "удержание",
                _ => "удержание недостачи",
            };
            let mut line = format!(
                "  {} · {what} {}",
                r.business_date.format("%d.%m"),
                format_som_signed(r.amount_tyiyn)
            );
            if !r.comment.trim().is_empty() {
                line.push_str(&format!(" — {}", r.comment.trim()));
            }
            if r.reversed {
                let why = rows
                    .iter()
                    .find(|x| x.reversal_of == Some(r.id))
                    .map(|x| x.comment.trim())
                    .filter(|c| !c.is_empty());
                line.push_str(&match why {
                    Some(w) => format!("\n    ↩️ отменено: {w}"),
                    None => "\n    ↩️ отменено".into(),
                });
            }
            lines.push(line);
        }
    }
    lines.push(String::new());
    lines.push("Отменённые в заработок не входят.".into());
    Ok(lines.join("\n"))
}

/// Свои работы: мастеру — замены (дата, госномер, услуга и масло), кассиру — смены (дата,
/// номер, расхождение при закрытии). Цены и суммы чеков не показываются.
async fn works_text(conn: &mut PgConnection, e: &Employee) -> AppResult<String> {
    let changes = sqlx::query!(
        r#"select s.business_date, v.plate as "plate?",
                  (select o.oil_text from oil_changes o where o.sale_id = s.id
                   order by o.created_at limit 1) as "oil?",
                  (select string_agg(sv.name, ', ' order by l.line_no)
                   from sale_lines l join services sv on sv.id = l.service_id
                   where l.sale_id = s.id) as "services?",
                  (select string_agg(p.name, ', ' order by l.line_no)
                   from sale_lines l join products p on p.id = l.product_id
                   where l.sale_id = s.id) as "goods?",
                  coalesce((select sum(r.total_tyiyn) from sales r where r.reversal_of = s.id), 0)
                    = -s.total_tyiyn and s.total_tyiyn <> 0 as "returned!"
           from sales s
           left join party_vehicles v on v.id = s.vehicle_id
           where s.branch_id = $1 and s.master_id = $2 and s.kind = 'sale'
           order by s.business_date desc, s.created_at desc
           limit $3"#,
        e.branch_id,
        e.id,
        WORKS
    )
    .fetch_all(&mut *conn)
    .await?;
    let shifts = sqlx::query!(
        r#"select s.number, s.business_date, c.diff_tyiyn as "diff?",
                  (c.id is not null
                   and not exists (select 1 from shift_reopens r where r.close_id = c.id)) as "closed!"
           from shifts s
           left join lateral (select x.id, x.diff_tyiyn from shift_closes x
                              where x.shift_id = s.id order by x.seq desc limit 1) c on true
           where s.branch_id = $1 and s.cashier_employee_id = $2
           order by s.business_date desc, s.opened_at desc
           limit $3"#,
        e.branch_id,
        e.id,
        WORKS
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut parts = vec![format!("🔧 Мои работы — {}", e.name)];
    if e.is_master || !changes.is_empty() {
        let mut lines = vec![format!("Замены (последние {WORKS}):")];
        if changes.is_empty() {
            lines.push("  пока нет".into());
        }
        for c in &changes {
            // Масло из масляной книжки; без неё (замена без машины) — товары чека, без цен.
            let oil = c
                .oil
                .clone()
                .filter(|o| !o.trim().is_empty())
                .or_else(|| c.goods.clone())
                .unwrap_or_default();
            let what = [c.services.clone().unwrap_or_default(), oil]
                .into_iter()
                .filter(|x| !x.trim().is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            let mut line = format!("  {}", c.business_date.format("%d.%m.%Y"));
            if let Some(p) = c.plate.as_deref().filter(|p| !p.is_empty()) {
                line.push_str(&format!(" · {p}"));
            }
            line.push_str(&format!(
                " · {}",
                if what.is_empty() {
                    "замена"
                } else {
                    &what
                }
            ));
            if c.returned {
                line.push_str(" (возврат)");
            }
            lines.push(line);
        }
        parts.push(lines.join("\n"));
    }
    if e.is_cashier || !shifts.is_empty() {
        let mut lines = vec![format!("Смены (последние {WORKS}):")];
        if shifts.is_empty() {
            lines.push("  пока нет".into());
        }
        for s in &shifts {
            let result = match (s.closed, s.diff) {
                (false, _) | (true, None) => "смена открыта".to_string(),
                (true, Some(0)) => "при закрытии сошлось".to_string(),
                (true, Some(d)) if d < 0 => format!("недостача {}", format_som(-d)),
                (true, Some(d)) => format!("излишек {}", format_som(d)),
            };
            lines.push(format!(
                "  {} · смена № {} · {result}",
                s.business_date.format("%d.%m.%Y"),
                s.number
            ));
        }
        parts.push(lines.join("\n"));
    }
    if parts.len() == 1 {
        parts.push("Работ пока нет.".into());
    }
    Ok(parts.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_by_button_and_command() {
        assert_eq!(section("💼 Сегодня"), Some(Section::Day));
        assert_eq!(section("💼 Мой заработок"), Some(Section::Day));
        assert_eq!(section("/месяц"), Some(Section::Month));
        assert_eq!(section("🔧 Мои работы"), Some(Section::Works));
        // «Мои машины» — клиентская кнопка, не работы.
        assert_eq!(section("🚗 Мои машины"), None);
        assert_eq!(section("сколько я заработал"), None);
    }

    #[test]
    fn months_parse_strictly() {
        assert_eq!(parse_month("2026-10"), NaiveDate::from_ymd_opt(2026, 10, 1));
        assert_eq!(parse_month("2026-13"), None);
        assert_eq!(parse_month("2026-10-05"), None);
    }

    #[test]
    fn service_fee_shows_count_and_rate() {
        let i = PayItem {
            kind: "service_fee".into(),
            count: 5,
            amount_tyiyn: 15_000,
        };
        assert_eq!(item_line(&i), "за замены: 5 × 30 с = 150 с");
    }
}
