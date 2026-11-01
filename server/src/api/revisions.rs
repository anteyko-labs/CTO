//! Ревизия склада: пересчёт по категориям и выравнивание остатков (SPEC-15).

use axum::extract::{Path, Query, State};
use axum::{Json, Router, routing};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::receipts::lock_products;
use crate::auth::{Ctx, CurrentUser};
use crate::domain::costing::cost_of;
use crate::domain::money::div_round;
use crate::domain::qty::qty_text;
use crate::error::{AppError, AppResult, invalid, overflow};
use crate::ops::{self, Movement, new_id};
use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/revisions", routing::get(list).post(create))
        .route("/revisions/{id}", routing::get(get_one))
        .route("/revisions/{id}/lines", routing::put(put_line))
        .route("/revisions/{id}/new-product", routing::post(new_product))
        .route(
            "/revisions/{id}/lines/{product_id}",
            routing::get(get_line).delete(delete_line),
        )
        .route("/revisions/{id}/post", routing::post(post_revision))
        .route("/revisions/{id}/cancel", routing::post(cancel))
}

#[derive(Serialize, Deserialize, Clone)]
pub struct RevisionHead {
    pub id: Uuid,
    pub number: i64,
    pub category_id: Option<Uuid>,
    pub category_name: Option<String>,
    pub status: String,
    pub comment: String,
    pub user_name: String,
    pub created_at: DateTime<Utc>,
    pub posted_at: Option<DateTime<Utc>>,
    pub counted: i64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct RevisionLine {
    pub product_id: Uuid,
    pub name: String,
    pub article: String,
    pub barcodes: Vec<String>,
    pub unit: String,
    pub container_ml: Option<i64>,
    /// Остаток на момент пересчёта строки (не пересчитана — сейчас); проведённая — тот же, из итога.
    pub expected_qty: i64,
    pub counted_qty: Option<i64>,
    /// Стоимость расхождения — только владельцу (инвариант 13).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_delta_tyiyn: Option<i64>,
    /// Масло: расхождение больше нормы (вопрос 32, ADR-052).
    #[serde(default)]
    pub over_norm: bool,
    /// Вязкость масла из карточки (`attrs.viscosity`) — в строке ревизии (SPEC-36 §13, ADR-123).
    #[serde(default)]
    pub viscosity: Option<String>,
    /// Кто записал строку последним: строку считают несколько человек с разных устройств
    /// (задача 003, Н14). Не посчитана — нет.
    #[serde(default)]
    pub counted_by: Option<String>,
    /// Кто сколько посчитал: «Айбек +2, Нурлан +1, Азиз = 5» — по журналу `revision_line_adds`,
    /// подряд идущие прибавки одного человека — одной записью (задача 003, Н14).
    #[serde(default)]
    pub counted_log: Vec<CountEntry>,
    /// Номер последней записи строки (`revision_line_adds.seq`, 0 — не трогали): экран показывает
    /// ответ записи или перечитывания, только если он новее показанного (задача 003, Н14).
    #[serde(default)]
    pub version: i64,
}

/// Запись журнала пересчёта строки: `add` — прибавил, `set` — записал итог, `clear` — убрал строку.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct CountEntry {
    pub by: String,
    /// Вход, который записал: у сотрудников свои входы (задача 005, Н7), а имена бывают одинаковые —
    /// прибавки складываются по входу, а не по имени.
    #[serde(default)]
    pub user_id: Option<Uuid>,
    pub mode: String,
    pub qty: i64,
}

/// Журнал строки и её версия (номер последней записи).
#[derive(Default)]
struct LineLog {
    entries: Vec<CountEntry>,
    version: i64,
}

/// Журнал пересчёта ревизии по товарам — в порядке записи (`seq`: вставка под замком черновика,
/// а не время начала транзакции); прибавки одного человека подряд складываются.
async fn count_log(
    conn: &mut PgConnection,
    id: Uuid,
) -> AppResult<std::collections::HashMap<Uuid, LineLog>> {
    let rows = sqlx::query!(
        r#"select a.product_id, a.mode, a.qty, a.seq, a.user_id, u.full_name
           from revision_line_adds a join users u on u.id = a.user_id
           where a.revision_id = $1 order by a.seq"#,
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut out: std::collections::HashMap<Uuid, LineLog> = Default::default();
    for r in rows {
        let line = out.entry(r.product_id).or_default();
        line.version = r.seq;
        let log = &mut line.entries;
        match log.last_mut() {
            Some(last)
                if r.mode == "add" && last.mode == "add" && last.user_id == Some(r.user_id) =>
            {
                last.qty = last.qty.saturating_add(r.qty);
            }
            _ => log.push(CountEntry {
                by: r.full_name,
                user_id: Some(r.user_id),
                mode: r.mode,
                qty: r.qty,
            }),
        }
    }
    Ok(out)
}

/// Запись в журнал пересчёта строки (только пополняется, I5); номер записи — новая версия строки.
async fn log_count(
    conn: &mut PgConnection,
    ctx: &Ctx,
    revision_id: Uuid,
    product_id: Uuid,
    mode: &str,
    qty: i64,
    op_id: Option<Uuid>,
) -> AppResult<i64> {
    Ok(sqlx::query_scalar!(
        r#"insert into revision_line_adds (id, revision_id, product_id, mode, qty, user_id, device_id, op_id)
           values ($1, $2, $3, $4, $5, $6, $7, $8) returning seq"#,
        new_id(),
        revision_id,
        product_id,
        mode,
        qty,
        ctx.user.id,
        ctx.device_id,
        op_id
    )
    .fetch_one(&mut *conn)
    .await?)
}

/// Версия строки сейчас: номер последней записи журнала (0 — не трогали).
async fn line_version(conn: &mut PgConnection, id: Uuid, product_id: Uuid) -> AppResult<i64> {
    Ok(sqlx::query_scalar!(
        r#"select coalesce(max(seq), 0) as "v!" from revision_line_adds
           where revision_id = $1 and product_id = $2"#,
        id,
        product_id
    )
    .fetch_one(&mut *conn)
    .await?)
}

#[derive(Serialize, Deserialize, Clone)]
pub struct RevisionOut {
    pub head: RevisionHead,
    pub lines: Vec<RevisionLine>,
    /// Норма расхождения по маслу, сотые доли процента от «должно быть».
    #[serde(default)]
    pub oil_norm_bp: i32,
    /// Номер последней записи пересчёта ревизии: строка, показанная на экране новее, при
    /// перечитывании не заменяется старой (задача 003, Н14).
    #[serde(default)]
    pub version: i64,
}

/// Расхождение по маслу больше нормы: |пересчитано − должно| > должно × норма.
pub fn over_norm(unit: &str, expected: i64, counted: Option<i64>, norm_bp: i32) -> bool {
    let Some(counted) = counted else { return false };
    if unit != "ml" {
        return false;
    }
    let diff = i128::from(counted) - i128::from(expected);
    diff.abs() * 10_000 > i128::from(expected.max(0)) * i128::from(norm_bp)
}

async fn head(conn: &mut PgConnection, branch_id: Uuid, id: Uuid) -> AppResult<RevisionHead> {
    sqlx::query_as!(
        RevisionHead,
        r#"select r.id, r.number, r.category_id, c.name as "category_name?", r.status, r.comment,
                  u.full_name as user_name, r.created_at, r.posted_at,
                  (select count(*) from revision_lines l where l.revision_id = r.id) as "counted!"
           from revisions r
           join users u on u.id = r.user_id
           left join categories c on c.id = r.category_id
           where r.id = $1 and r.branch_id = $2"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)
}

async fn load(conn: &mut PgConnection, user: &CurrentUser, id: Uuid) -> AppResult<RevisionOut> {
    let h = head(conn, user.branch_id, id).await?;
    let owner = user.is_owner();
    let norm = crate::api::receipts::oil_norm_bp(conn, user.branch_id).await?;
    let mut log = count_log(conn, id).await?;
    let version = log.values().map(|l| l.version).max().unwrap_or(0);
    let lines = if h.status == "posted" {
        sqlx::query!(
            r#"select p.id, p.name, p.article, p.unit, p.container_ml,
                      nullif(p.attrs->>'viscosity', '') as viscosity,
                      coalesce((select array_agg(b.code order by b.code) from product_barcodes b
                                where b.product_id = p.id), '{}') as "barcodes!",
                      x.expected_qty, x.counted_qty, x.value_delta_tyiyn, u.full_name as "counted_by?"
               from revision_results x join products p on p.id = x.product_id
               left join revision_lines l on l.revision_id = x.revision_id and l.product_id = x.product_id
               left join users u on u.id = l.user_id
               where x.revision_id = $1 order by p.name"#,
            id
        )
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|r| {
            let lg = log.remove(&r.id).unwrap_or_default();
            RevisionLine {
            product_id: r.id,
            name: r.name,
            article: r.article,
            barcodes: r.barcodes,
            container_ml: r.container_ml,
            expected_qty: r.expected_qty,
            over_norm: over_norm(&r.unit, r.expected_qty, Some(r.counted_qty), norm),
            counted_qty: Some(r.counted_qty),
            value_delta_tyiyn: owner.then_some(r.value_delta_tyiyn),
            unit: r.unit,
            viscosity: r.viscosity,
            counted_by: r.counted_by,
            counted_log: lg.entries,
            version: lg.version,
        }})
        .collect()
    } else {
        // Черновик: товары категории (или все) и пересчитанное, сверху — уже посчитанные.
        sqlx::query!(
            r#"select p.id, p.name, p.article, p.unit, p.container_ml,
                      nullif(p.attrs->>'viscosity', '') as viscosity,
                      coalesce((select array_agg(b.code order by b.code) from product_barcodes b
                                where b.product_id = p.id), '{}') as "barcodes!",
                      coalesce(l.stock_at_count, bp.stock_qty, 0) as "expected!", l.counted_qty as "counted?",
                      u.full_name as "counted_by?"
               from products p
               left join branch_products bp on bp.product_id = p.id and bp.branch_id = $2
               left join revision_lines l on l.product_id = p.id and l.revision_id = $1
               left join users u on u.id = l.user_id
               where not p.archived
                 and ($3::uuid is null or p.category_id = $3)
                 and (l.product_id is not null or coalesce(bp.stock_qty, 0) <> 0 or $3::uuid is not null)
               order by (l.product_id is null), p.name
               limit 3000"#,
            id,
            user.branch_id,
            h.category_id
        )
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(|r| {
            let lg = log.remove(&r.id).unwrap_or_default();
            RevisionLine {
            product_id: r.id,
            name: r.name,
            article: r.article,
            barcodes: r.barcodes,
            over_norm: over_norm(&r.unit, r.expected, r.counted, norm),
            unit: r.unit,
            container_ml: r.container_ml,
            expected_qty: r.expected,
            counted_qty: r.counted,
            value_delta_tyiyn: None,
            viscosity: r.viscosity,
            counted_by: r.counted_by,
            counted_log: lg.entries,
            version: lg.version,
        }})
        .collect()
    };
    Ok(RevisionOut {
        head: h,
        lines,
        oil_norm_bp: norm,
        version,
    })
}

async fn list(
    State(state): State<AppState>,
    user: CurrentUser,
) -> AppResult<Json<Vec<RevisionHead>>> {
    let mut conn = state.pool.acquire().await?;
    Ok(Json(list_data(&mut conn, &user).await?))
}

/// Последние 100 ревизий — экран и бот владельца (SPEC-24).
pub(crate) async fn list_data(
    conn: &mut PgConnection,
    user: &CurrentUser,
) -> AppResult<Vec<RevisionHead>> {
    let rows = sqlx::query_as!(
        RevisionHead,
        r#"select r.id, r.number, r.category_id, c.name as "category_name?", r.status, r.comment,
                  u.full_name as user_name, r.created_at, r.posted_at,
                  (select count(*) from revision_lines l where l.revision_id = r.id) as "counted!"
           from revisions r
           join users u on u.id = r.user_id
           left join categories c on c.id = r.category_id
           where r.branch_id = $1
           order by r.created_at desc limit 100"#,
        user.branch_id
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows)
}

async fn get_one(
    State(state): State<AppState>,
    user: CurrentUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<RevisionOut>> {
    // Один снимок базы: итог строки и её версия (журнал) — на один момент, иначе показ мог бы
    // получить итог с чужой записью и версию до своей (задача 003, Н14).
    let mut tx = state.pool.begin().await?;
    snapshot(&mut tx).await?;
    let out = load(&mut tx, &user, id).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Чтение на один момент: все запросы транзакции видят одно и то же состояние базы.
async fn snapshot(conn: &mut PgConnection) -> AppResult<()> {
    sqlx::query!("set transaction isolation level repeatable read, read only")
        .execute(&mut *conn)
        .await?;
    Ok(())
}

#[derive(Deserialize)]
struct CreateReq {
    category_id: Option<Uuid>,
    #[serde(default)]
    comment: String,
    // Прежний флаг `reuse_draft` («продолжить открытый черновик того же охвата») не читается:
    // так теперь всегда — две ревизии одного товара вычли бы его остаток дважды (задача 003, Н14).
}

async fn create(
    State(state): State<AppState>,
    ctx: Ctx,
    Json(req): Json<CreateReq>,
) -> AppResult<Json<RevisionOut>> {
    let mut tx = state.pool.begin().await?;
    // Ревизия — право администратора `revision` (ADR-097).
    crate::auth::require_permission(&mut tx, &ctx.user, "revision").await?;
    if let Some(cid) = req.category_id {
        sqlx::query_scalar!("select id from categories where id = $1", cid)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| invalid("категория не найдена"))?;
    }
    // Замок «черновик филиала» — при любом начале ревизии (и со старой версии экрана):
    // устройства, начавшие одновременно, ждут друг друга и находят черновик первого.
    lock_branch_drafts(&mut tx, ctx.user.branch_id).await?;
    // Охваты пересекаются: «весь склад» включает любую категорию. Две ревизии одного товара
    // вычли бы его остаток дважды — открытый черновик того же охвата или «весь склад»
    // продолжаем; «весь склад» при открытой категории — 409 «закончите или присоединитесь».
    let open = sqlx::query!(
        r#"select r.id, r.category_id, c.name as "category_name?"
           from revisions r left join categories c on c.id = r.category_id
           where r.branch_id = $1 and r.status = 'draft'
             and (r.category_id is null or $2::uuid is null or r.category_id = $2)
           order by (r.category_id is not distinct from $2) desc, (r.category_id is null) desc,
                    r.created_at desc
           limit 1"#,
        ctx.user.branch_id,
        req.category_id
    )
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(o) = open {
        if o.category_id.is_some() && o.category_id != req.category_id {
            return Err(AppError::ConflictCode(
                "revision_open",
                format!(
                    "Идёт ревизия «{}» — закончите или присоединитесь",
                    o.category_name.unwrap_or_default()
                ),
            ));
        }
        let out = load(&mut tx, &ctx.user, o.id).await?;
        tx.commit().await?;
        return Ok(Json(out));
    }
    let number = ops::next_counter(&mut tx, ctx.user.branch_id, "revision").await?;
    let id = new_id();
    sqlx::query!(
        r#"insert into revisions (id, branch_id, number, category_id, comment, user_id, device_id)
           values ($1, $2, $3, $4, $5, $6, $7)"#,
        id,
        ctx.user.branch_id,
        number,
        req.category_id,
        req.comment.trim(),
        ctx.user.id,
        ctx.device_id
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "revision.create",
        "revision",
        Some(id),
        json!({ "number": number }),
    )
    .await?;
    let out = load(&mut tx, &ctx.user, id).await?;
    tx.commit().await?;
    Ok(Json(out))
}

/// Замок «черновики филиала» до конца транзакции: начало ревизии, запись пересчёта и проведение
/// идут по очереди. Без него проведение черновика X и запись того же товара в черновик Y (товар
/// перенесли в другую категорию, пока открыты оба) разминулись бы: проверка пересечения при
/// проведении X строки в Y ещё не видит, а строка Y берёт остаток до движения X — недостача
/// вычлась бы дважды (задача 003, Н14). Берётся раньше замка строки ревизии.
async fn lock_branch_drafts(conn: &mut PgConnection, branch_id: Uuid) -> AppResult<()> {
    sqlx::query!(
        "select pg_advisory_xact_lock(hashtextextended('revision.draft:' || $1::text, 0))",
        branch_id.to_string()
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Товар посчитан и в другом открытом черновике филиала (заведён до проверки охватов или товар
/// перенесли в другую категорию): провести оба — вычесть остаток дважды. Под замком
/// `lock_branch_drafts`: черновик, который сейчас проводят, дождётся конца проведения.
async fn counted_elsewhere(
    conn: &mut PgConnection,
    branch_id: Uuid,
    id: Uuid,
    product_ids: &[Uuid],
) -> AppResult<()> {
    let twice = sqlx::query!(
        r#"select p.name, r.number from revision_lines l
           join revisions r on r.id = l.revision_id
           join products p on p.id = l.product_id
           where r.branch_id = $1 and r.status = 'draft' and r.id <> $2 and l.product_id = any($3)
           order by p.name, r.number limit 1"#,
        branch_id,
        id,
        product_ids
    )
    .fetch_optional(&mut *conn)
    .await?;
    match twice {
        Some(t) => Err(AppError::ConflictCode(
            "revision_overlap",
            format!(
                "«{}» посчитан и в открытой ревизии № {} — проведите или отмените одну из них, иначе остаток вычтется дважды",
                t.name, t.number
            ),
        )),
        None => Ok(()),
    }
}

/// Черновик ревизии этого филиала, заблокированный для правки.
async fn lock_draft(conn: &mut PgConnection, branch_id: Uuid, id: Uuid) -> AppResult<()> {
    let status = sqlx::query_scalar!(
        "select status from revisions where id = $1 and branch_id = $2 for update",
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    if status != "draft" {
        return Err(AppError::Conflict(
            "ревизия уже проведена или отменена".into(),
        ));
    }
    Ok(())
}

/// Черновик для записи пересчёта: ревизию провели или отменили, пока на устройстве считали, —
/// 409 `revision_closed` с тем, кто провёл, и что делать с отсканированным (задача 003, Н14).
async fn lock_draft_for_count(conn: &mut PgConnection, branch_id: Uuid, id: Uuid) -> AppResult<()> {
    let r = sqlx::query!(
        r#"select r.status, r.number, u.full_name as "posted_by?"
           from revisions r left join users u on u.id = r.posted_by
           where r.id = $1 and r.branch_id = $2 for update of r"#,
        id,
        branch_id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(AppError::NotFound)?;
    if r.status == "draft" {
        return Ok(());
    }
    let what = if r.status == "posted" {
        match r.posted_by {
            Some(who) => format!("уже проведена ({who})"),
            None => "уже проведена".into(),
        }
    } else {
        "отменена".into()
    };
    Err(AppError::ConflictCode(
        "revision_closed",
        format!(
            "Ревизия № {} {what}; ваш скан не учтён — проведите оприходование или новую ревизию",
            r.number
        ),
    ))
}

#[derive(Deserialize)]
struct LineReq {
    product_id: Uuid,
    counted_qty: i64,
    /// `set` — записать пересчитанное (по умолчанию, экран «Ревизия» шлёт итог строки);
    /// `add` — прибавить к уже посчитанному: товар лежит в нескольких местах (SPEC-20, ADR-059).
    #[serde(default)]
    mode: CountMode,
    /// Повтор того же ввода (обрыв связи, двойное нажатие) не прибавит полку дважды:
    /// сервер вернёт прежний ответ (ADR-013, ADR-062). Без `op_id` — как раньше.
    #[serde(default)]
    op_id: Option<Uuid>,
    /// `set`: итог строки, который человек видел на экране, когда начал ввод, плюс его
    /// собственные записи после этого. Пока он вводил, другие прибавили — 409 `count_changed`
    /// «сейчас N», а не затирание чужих сканов (задача 003, Н14).
    #[serde(default)]
    expected_qty: Option<i64>,
    /// `set`: человек видел строку непосчитанной — строки быть не должно. «Не посчитана» и
    /// «посчитано 0» («не нашли») — разное: 0 другого человека не затирается молча.
    #[serde(default)]
    expected_empty: bool,
}

/// Что человек видел в строке, когда начал ввод: `None` — без проверки (старая версия экрана),
/// `Some(None)` — строки не было, `Some(Some(n))` — посчитано n.
fn expected_of(qty: Option<i64>, empty: bool) -> Option<Option<i64>> {
    if empty { Some(None) } else { qty.map(Some) }
}

/// «= итог» без `expected_qty` шлёт только экран до обновления: строку, которую за последние
/// 10 минут прибавляли с других устройств или другим входом (на общем планшете сотрудники входят
/// по очереди, задача 005, Н7), он затёр бы — 409 «обновите страницу».
async fn outdated_screen(
    conn: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    product_id: Uuid,
) -> AppResult<()> {
    let fresh = sqlx::query_scalar!(
        r#"select exists(select 1 from revision_line_adds
                         where revision_id = $1 and product_id = $2 and mode = 'add'
                           and (device_id <> $3 or user_id <> $4)
                           and created_at > clock_timestamp() - interval '10 minutes') as "fresh!""#,
        id,
        product_id,
        ctx.device_id,
        ctx.user.id
    )
    .fetch_one(&mut *conn)
    .await?;
    if fresh {
        return Err(AppError::ConflictCode(
            "client_outdated",
            "Обновите страницу: на кассе старая версия программы — строку сейчас считают с других устройств".into(),
        ));
    }
    Ok(())
}

#[derive(Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum CountMode {
    #[default]
    Set,
    Add,
}

/// Сколько уже пересчитано по товару в черновике: экран «Работа с товарами» показывает это
/// перед вводом и спрашивает, прибавить или заменить.
async fn get_line(
    State(state): State<AppState>,
    user: CurrentUser,
    Path((id, product_id)): Path<(Uuid, Uuid)>,
) -> AppResult<Json<serde_json::Value>> {
    let mut conn = state.pool.begin().await?;
    // Итог и версия — из одного снимка: устаревший итог не получит свежую версию.
    snapshot(&mut conn).await?;
    // Чужой филиал — как будто строки нет.
    let mine = sqlx::query_scalar!(
        "select id from revisions where id = $1 and branch_id = $2",
        id,
        user.branch_id
    )
    .fetch_optional(&mut *conn)
    .await?;
    if mine.is_none() {
        return Ok(Json(json!({ "counted_qty": null, "version": 0 })));
    }
    let counted = counted_now(&mut conn, id, product_id).await?;
    let version = line_version(&mut conn, id, product_id).await?;
    conn.commit().await?;
    Ok(Json(json!({ "counted_qty": counted, "version": version })))
}

/// Посчитано по строке сейчас; строки нет — `None` (не путать с посчитанным 0).
async fn counted_now(
    conn: &mut PgConnection,
    id: Uuid,
    product_id: Uuid,
) -> AppResult<Option<i64>> {
    Ok(sqlx::query_scalar!(
        "select counted_qty from revision_lines where revision_id = $1 and product_id = $2",
        id,
        product_id
    )
    .fetch_optional(&mut *conn)
    .await?)
}

/// Итог строки изменился, пока человек вводил: экран перечитает строку и переспросит.
fn count_changed(now: Option<i64>, total: Option<i64>, unit: &str) -> AppError {
    let ask = match total {
        Some(t) => format!("Записать итог {}?", qty_text(t, unit)),
        None => "Всё равно убрать строку?".into(),
    };
    let now = match now {
        Some(n) => format!("сейчас {}", qty_text(n, unit)),
        None => "сейчас строка не посчитана".into(),
    };
    AppError::ConflictCode(
        "count_changed",
        format!("Пока вы вводили, итог строки изменили: {now}. {ask}"),
    )
}

async fn put_line(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<LineReq>,
) -> AppResult<Json<serde_json::Value>> {
    if !(0..=1_000_000_000).contains(&req.counted_qty) {
        return Err(invalid("пересчитано: от 0"));
    }
    const KIND: &str = "revision.line";
    let mut tx = state.pool.begin().await?;
    crate::auth::require_permission(&mut tx, &ctx.user, "revision").await?;
    if let Some(op_id) = req.op_id
        && let Some(done) = ops::begin_op(&mut tx, &ctx, op_id, KIND).await?
    {
        return Ok(Json(done));
    }
    lock_branch_drafts(&mut tx, ctx.user.branch_id).await?;
    lock_draft_for_count(&mut tx, ctx.user.branch_id, id).await?;
    let product = sqlx::query!(
        "select archived, unit from products where id = $1",
        req.product_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| invalid("товар не найден"))?;
    if product.archived {
        return Err(invalid("товар в архиве: считайте основную карточку"));
    }
    // Товар уже посчитан в другом открытом черновике: вторая строка вычла бы остаток дважды.
    // Тот черновик проводят — замок выше ждёт конца проведения, остаток ниже — уже после него.
    counted_elsewhere(&mut tx, ctx.user.branch_id, id, &[req.product_id]).await?;
    let add = req.mode == CountMode::Add;
    // Записи строк одной ревизии идут по очереди (замок черновика выше): посчитанное сейчас —
    // точное, проверка итога на экране не гоняется с чужой прибавкой.
    if !add {
        match expected_of(req.expected_qty, req.expected_empty) {
            Some(seen) => {
                let now = counted_now(&mut tx, id, req.product_id).await?;
                if now != seen {
                    return Err(count_changed(now, Some(req.counted_qty), &product.unit));
                }
            }
            None => outdated_screen(&mut tx, &ctx, id, req.product_id).await?,
        }
    }
    // Остаток в момент пересчёта: продажи между пересчётом и проведением не станут «излишком».
    let stock_now = sqlx::query_scalar!(
        "select stock_qty from branch_products where branch_id = $1 and product_id = $2",
        ctx.user.branch_id,
        req.product_id
    )
    .fetch_optional(&mut *tx)
    .await?
    .unwrap_or(0);
    // `set` — пересчёт строки целиком: остаток берём на этот момент.
    // `add` — дочитали другую полку: прибавляем в одном запросе (два устройства не потеряют
    // друг друга) и оставляем остаток первого пересчёта. Расхождение = пересчитано − остаток
    // «на момент пересчёта», а пересчёт по частям растянут во времени, и точного момента нет.
    // Первый остаток верен, когда проданное между пересчётами взяли с уже посчитанной полки —
    // так обычно и бывает: считают с витрины, продают с витрины. Если бы брали остаток второго
    // пересчёта, такая продажа вычлась бы дважды (её нет на полке и нет в остатке) — ложная
    // недостача. Продажа с ещё не посчитанной полки, наоборот, даст недостачу на проданное;
    // поэтому одну позицию лучше пересчитывать подряд (SPEC-15, ADR-059).
    let counted = sqlx::query_scalar!(
        r#"insert into revision_lines (revision_id, product_id, counted_qty, user_id, stock_at_count)
           values ($1, $2, $3, $4, $5)
           on conflict (revision_id, product_id)
           do update set counted_qty = case when $6 then revision_lines.counted_qty + excluded.counted_qty
                                            else excluded.counted_qty end,
                         user_id = excluded.user_id, updated_at = now(),
                         stock_at_count = case when $6 then coalesce(revision_lines.stock_at_count, excluded.stock_at_count)
                                               else excluded.stock_at_count end
           returning counted_qty"#,
        id,
        req.product_id,
        req.counted_qty,
        ctx.user.id,
        stock_now,
        add
    )
    .fetch_one(&mut *tx)
    .await?;
    if counted > 1_000_000_000 {
        return Err(invalid("пересчитано слишком много"));
    }
    let version = log_count(
        &mut tx,
        &ctx,
        id,
        req.product_id,
        if add { "add" } else { "set" },
        req.counted_qty,
        req.op_id,
    )
    .await?;
    let out = json!({ "ok": true, "counted_qty": counted, "version": version });
    if let Some(op_id) = req.op_id {
        ops::finish_op(&mut tx, &ctx, op_id, KIND, &out).await?;
    }
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct NewProductReq {
    /// Найдено на полке: штуки, миллилитры (масло), граммы.
    counted_qty: i64,
    /// Карточка товара — те же поля, что `POST /products`; `op_id` — операции целиком.
    #[serde(flatten)]
    product: crate::api::catalog::CreateProductReq,
    /// Прошлые нажатия «Завести» в этом окне, на которые не пришёл ответ (обрыв связи), а ввод
    /// потом изменили: если одно из них уже завело карточку — ответ того нажатия с `earlier`,
    /// второй товар не заводится (задача 003, Н14).
    #[serde(default)]
    prev_op_ids: Vec<Uuid>,
}

/// Товара нет в программе, а на полке он есть (SPEC-29 §10): карточка заводится и сразу
/// попадает в черновик ревизии с найденным количеством — одна операция, одна транзакция.
/// Излишек при проведении оценивается по закупке из карточки (`initial_cost_tyiyn`, SPEC-20).
async fn new_product(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<NewProductReq>,
) -> AppResult<Json<serde_json::Value>> {
    const KIND: &str = "revision.new_product";
    if !(0..=1_000_000_000).contains(&req.counted_qty) {
        return Err(invalid("найдено: от 0"));
    }
    // Без цены товар ушёл бы с кассы за 0 с (проверка 20.10.2026).
    if req.product.sale_price_tyiyn <= 0 {
        return Err(invalid("укажите цену продажи"));
    }
    let op_id = req.product.op_id;
    let mut tx = state.pool.begin().await?;
    crate::auth::require_permission(&mut tx, &ctx.user, "revision").await?;
    if let Some(done) = ops::begin_op(&mut tx, &ctx, op_id, KIND).await? {
        return Ok(Json(done));
    }
    if req.prev_op_ids.len() > 50 {
        return Err(invalid("слишком много прошлых нажатий"));
    }
    for prev in req.prev_op_ids.iter().filter(|p| **p != op_id) {
        if let Some(mut done) =
            ops::begin_op::<serde_json::Value>(&mut tx, &ctx, *prev, KIND).await?
        {
            done["earlier"] = json!(true);
            return Ok(Json(done));
        }
    }
    // Тот же порядок замков, что у записи пересчёта и проведения (новый товар в другом черновике
    // посчитан быть не может — проверять пересечение нечего).
    lock_branch_drafts(&mut tx, ctx.user.branch_id).await?;
    lock_draft_for_count(&mut tx, ctx.user.branch_id, id).await?;
    let mut product = req.product;
    // Своя операция карточки: повтор всей операции отсекает `op_id` выше.
    product.op_id = new_id();
    let p = crate::api::catalog::create_product_tx(&mut tx, &ctx, product).await?;
    sqlx::query!(
        r#"insert into revision_lines (revision_id, product_id, counted_qty, user_id, stock_at_count)
           values ($1, $2, $3, $4, 0)"#,
        id,
        p.id,
        req.counted_qty,
        ctx.user.id
    )
    .execute(&mut *tx)
    .await?;
    let version = log_count(&mut tx, &ctx, id, p.id, "add", req.counted_qty, Some(op_id)).await?;
    ops::audit(
        &mut tx,
        &ctx,
        KIND,
        "revision",
        Some(id),
        json!({ "product_id": p.id, "name": p.name, "counted_qty": req.counted_qty }),
    )
    .await?;
    let out =
        json!({ "ok": true, "product": p, "counted_qty": req.counted_qty, "version": version });
    ops::finish_op(&mut tx, &ctx, op_id, KIND, &out).await?;
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
struct DeleteLineQuery {
    /// Итог строки на экране, когда поле очистили (плюс свои записи после): пока очищали, другие
    /// прибавили или записали 0 — 409.
    expected_qty: Option<i64>,
    /// Человек видел строку непосчитанной: её успели посчитать — не убирать молча.
    #[serde(default)]
    expected_empty: bool,
    /// Повтор того же «убрать строку» (обрыв связи) — прежний ответ, а не 409 по чужой свежей
    /// прибавке (ADR-062, задача 003, Н14).
    op_id: Option<Uuid>,
}

async fn delete_line(
    State(state): State<AppState>,
    ctx: Ctx,
    Path((id, product_id)): Path<(Uuid, Uuid)>,
    Query(q): Query<DeleteLineQuery>,
) -> AppResult<Json<serde_json::Value>> {
    const KIND: &str = "revision.line_delete";
    let mut tx = state.pool.begin().await?;
    crate::auth::require_permission(&mut tx, &ctx.user, "revision").await?;
    if let Some(op_id) = q.op_id
        && let Some(done) = ops::begin_op(&mut tx, &ctx, op_id, KIND).await?
    {
        return Ok(Json(done));
    }
    lock_draft_for_count(&mut tx, ctx.user.branch_id, id).await?;
    if let Some(seen) = expected_of(q.expected_qty, q.expected_empty) {
        // Строки уже нет — убирать нечего, не ошибка. Есть, но не та, что видели (и посчитанный
        // другим 0 — тоже строка) — переспросить.
        if let Some(now) = counted_now(&mut tx, id, product_id).await?
            && seen != Some(now)
        {
            let unit = sqlx::query_scalar!("select unit from products where id = $1", product_id)
                .fetch_optional(&mut *tx)
                .await?
                .unwrap_or_default();
            return Err(count_changed(Some(now), None, &unit));
        }
    } else {
        // Без сверки очищает только экран до обновления: свежие чужие прибавки он стёр бы.
        outdated_screen(&mut tx, &ctx, id, product_id).await?;
    }
    let removed = sqlx::query_scalar!(
        "delete from revision_lines where revision_id = $1 and product_id = $2 returning counted_qty",
        id,
        product_id
    )
    .fetch_optional(&mut *tx)
    .await?;
    // Черновик правится, но кто и что убрал из пересчёта — в журнал (инвариант 5).
    let version = if let Some(counted) = removed {
        let v = log_count(&mut tx, &ctx, id, product_id, "clear", 0, q.op_id).await?;
        ops::audit(
            &mut tx,
            &ctx,
            KIND,
            "revision",
            Some(id),
            json!({ "product_id": product_id, "counted_qty": counted }),
        )
        .await?;
        v
    } else {
        line_version(&mut tx, id, product_id).await?
    };
    let out = json!({ "ok": true, "counted_qty": null, "version": version });
    if let Some(op_id) = q.op_id {
        ops::finish_op(&mut tx, &ctx, op_id, KIND, &out).await?;
    }
    tx.commit().await?;
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct PostReq {
    pub op_id: Uuid,
    #[serde(default)]
    pub comment: String,
    /// Провести, хотя другие ещё считают (за последние 60 с были записи строк): владелец
    /// подтвердил предупреждение «Ещё считают: …» (задача 003, Н14).
    #[serde(default)]
    pub force: bool,
}

/// Проведение: остатки выравниваются по пересчитанному, расхождение — в себестоимость (SPEC-15).
pub async fn post_revision_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    req: PostReq,
) -> AppResult<RevisionOut> {
    const KIND: &str = "revision.post";
    if let Some(done) = ops::begin_op(conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    if req.comment.trim().is_empty() {
        return Err(invalid("укажите причину или кто пересчитывал"));
    }
    let branch_id = ctx.user.branch_id;
    // Замок черновиков филиала — до конца проведения: запись того же товара в другой черновик
    // ждёт и потом видит эту ревизию проведённой, а остаток — после её движения.
    lock_branch_drafts(conn, branch_id).await?;
    lock_draft(conn, branch_id, id).await?;
    let lines = sqlx::query!(
        "select product_id, counted_qty, stock_at_count from revision_lines where revision_id = $1",
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    if lines.is_empty() {
        return Err(invalid("ничего не пересчитано"));
    }
    let ids: Vec<Uuid> = lines.iter().map(|l| l.product_id).collect();
    // Товар посчитан и в другом открытом черновике — перепроверка под замком черновиков.
    counted_elsewhere(conn, branch_id, id, &ids).await?;
    // Другие ещё считают (за последние 60 с были записи строк) — спросить владельца: скан,
    // пришедший после проведения, не учтётся.
    if !req.force {
        let busy = sqlx::query!(
            r#"select u.full_name, extract(epoch from clock_timestamp() - max(a.created_at))::bigint as "ago!"
               from revision_line_adds a join users u on u.id = a.user_id
               where a.revision_id = $1 and a.user_id <> $2
                 and a.created_at > clock_timestamp() - interval '60 seconds'
               group by u.full_name order by max(a.created_at) desc"#,
            id,
            ctx.user.id
        )
        .fetch_all(&mut *conn)
        .await?;
        if !busy.is_empty() {
            let who: Vec<String> = busy
                .iter()
                .map(|b| format!("{} {} с назад", b.full_name, b.ago.max(1)))
                .collect();
            return Err(AppError::ConflictCode(
                "still_counting",
                format!("Ещё считают: {}. Провести?", who.join(", ")),
            ));
        }
    }
    lock_products(conn, branch_id, ids.iter().copied()).await?;
    // Карточку могли объединить или убрать в архив после пересчёта: её остаток уже на основной,
    // и «излишек» по дублю удвоил бы склад.
    let gone = sqlx::query_scalar!(
        r#"select name from products where id = any($1) and archived order by name limit 1"#,
        &ids[..]
    )
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(name) = gone {
        return Err(AppError::Conflict(format!(
            "«{name}» объединили или убрали в архив после пересчёта — удалите строку и пересчитайте основную карточку"
        )));
    }
    let (mut shortage, mut surplus) = (0i64, 0i64);
    for l in &lines {
        let pool = ops::lock_pool(conn, branch_id, l.product_id).await?;
        let delta = l
            .counted_qty
            .checked_sub(l.stock_at_count.unwrap_or(pool.qty))
            .ok_or_else(overflow)?;
        let value = if delta < 0 {
            // Недостача уходит по средней, как продажа (ADR-011).
            -cost_of(-delta, &pool).ok_or_else(overflow)?
        } else if delta > 0 {
            // Излишек — по средней, а при пустом остатке — по последней закупке.
            let per = if pool.qty > 0 && pool.value > 0 {
                (pool.value, pool.qty)
            } else {
                (pool.last_cost, pool.last_qty.max(1))
            };
            div_round(i128::from(delta) * i128::from(per.0), i128::from(per.1))
                .ok_or_else(overflow)?
        } else {
            0
        };
        if delta != 0 {
            ops::apply_movement(
                conn,
                Movement {
                    branch_id,
                    product_id: l.product_id,
                    qty_delta: delta,
                    value_delta: value,
                    doc_type: "revision",
                    doc_id: id,
                },
            )
            .await?;
        }
        if value < 0 {
            shortage = shortage.saturating_sub(value);
        } else {
            surplus = surplus.saturating_add(value);
        }
        sqlx::query!(
            r#"insert into revision_results (revision_id, product_id, expected_qty, counted_qty,
                                             qty_delta, value_delta_tyiyn)
               values ($1, $2, $3, $4, $5, $6)"#,
            id,
            l.product_id,
            l.stock_at_count.unwrap_or(pool.qty),
            l.counted_qty,
            delta,
            value
        )
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query!(
        r#"update revisions set status = 'posted', posted_by = $2, posted_at = now(),
             comment = case when comment = '' then $3 else comment || ' · ' || $3 end
           where id = $1"#,
        id,
        ctx.user.id,
        req.comment.trim()
    )
    .execute(&mut *conn)
    .await?;
    let number = sqlx::query_scalar!("select number from revisions where id = $1", id)
        .fetch_one(&mut *conn)
        .await?;
    let out = load(conn, &ctx.user, id).await?;
    let over = out.lines.iter().filter(|l| l.over_norm).count();
    ops::audit(
        conn,
        ctx,
        KIND,
        "revision",
        Some(id),
        json!({ "number": number, "lines": lines.len(), "shortage": shortage, "surplus": surplus, "over_norm": over }),
    )
    .await?;
    ops::finish_op(conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}

async fn post_revision(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<PostReq>,
) -> AppResult<Json<RevisionOut>> {
    let mut tx = state.pool.begin().await?;
    let out = post_revision_tx(&mut tx, &ctx, id, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

async fn cancel(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    let mut tx = state.pool.begin().await?;
    crate::auth::require_permission(&mut tx, &ctx.user, "revision").await?;
    lock_draft(&mut tx, ctx.user.branch_id, id).await?;
    sqlx::query!(
        "update revisions set status = 'cancelled' where id = $1",
        id
    )
    .execute(&mut *tx)
    .await?;
    ops::audit(
        &mut tx,
        &ctx,
        "revision.cancel",
        "revision",
        Some(id),
        json!({}),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(json!({ "ok": true })))
}
