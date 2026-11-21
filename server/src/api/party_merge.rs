//! Объединение карточек клиентов и поставщиков (задача 043).
//!
//! Несколько карточек одного клиента (например, агенты одной фирмы) сливаются в одну: долг,
//! аванс и баллы переходят встречными записями (журналы не переписываются, I4), работники,
//! машины, кабинет и Telegram — на основную карточку, влитые уходят в архив с `merged_into`.
//! Чеки и замены остаются за влитыми и видны в карточке основной.

use axum::extract::{Path, State};
use axum::{Json, Router, routing};
use serde::Deserialize;
use serde_json::json;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::api::parties::{PartyOut, load_party, person_name};
use crate::auth::Ctx;
use crate::error::{AppError, AppResult, invalid};
use crate::ops::{self, new_id};
use crate::state::AppState;

/// Вид документа у записей переноса в журнале долга и баллов.
pub const MERGE_DOC: &str = "merge";
/// Больше карточек за раз не объединяют: окно выбора показывает столько же.
const MAX_DUPLICATES: usize = 20;

pub fn routes() -> Router<AppState> {
    Router::new().route("/parties/{id}/merge", routing::post(merge_parties))
}

#[derive(Deserialize)]
pub struct MergeReq {
    pub op_id: Uuid,
    /// Карточки, которые вливаются в эту.
    pub duplicate_ids: Vec<Uuid>,
    /// Вид итоговой карточки: `person` или `company`; без поля — как у основной.
    #[serde(default)]
    pub kind: Option<String>,
    /// Название итоговой карточки (у фирмы); пусто — прежнее.
    #[serde(default)]
    pub name: Option<String>,
    /// Имя и фамилия, если итоговая карточка — человек.
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub last_name: Option<String>,
}

async fn merge_parties(
    State(state): State<AppState>,
    ctx: Ctx,
    Path(id): Path<Uuid>,
    Json(req): Json<MergeReq>,
) -> AppResult<Json<PartyOut>> {
    let mut tx = state.pool.begin().await?;
    let out = merge_tx(&mut tx, &ctx, id, req).await?;
    tx.commit().await?;
    Ok(Json(out))
}

struct Card {
    id: Uuid,
    role: String,
    kind: String,
    name: String,
    phone: String,
    inn: String,
    merged_into: Option<Uuid>,
}

/// Объединение одной транзакцией вызывающего (I7).
pub async fn merge_tx(
    conn: &mut PgConnection,
    ctx: &Ctx,
    id: Uuid,
    req: MergeReq,
) -> AppResult<PartyOut> {
    const KIND: &str = "party.merge";
    if !ctx.user.is_owner() {
        return Err(AppError::Forbidden);
    }
    let mut dups = req.duplicate_ids.clone();
    dups.sort();
    dups.dedup();
    if dups.is_empty() {
        return Err(invalid("выберите карточки для объединения"));
    }
    if dups.len() > MAX_DUPLICATES {
        return Err(invalid("за раз — не больше 20 карточек"));
    }
    if dups.contains(&id) {
        return Err(AppError::Conflict(
            "карточку нельзя влить в саму себя".into(),
        ));
    }
    if req
        .kind
        .as_deref()
        .is_some_and(|k| !matches!(k, "person" | "company"))
    {
        return Err(invalid("вид клиента: физлицо или юрлицо"));
    }
    if let Some(done) = ops::begin_op(&mut *conn, ctx, req.op_id, KIND).await? {
        return Ok(done);
    }

    // Все карточки блокируются в порядке id: встречные объединения не ждут друг друга вечно.
    let mut all: Vec<Uuid> = dups.clone();
    all.push(id);
    all.sort();
    let rows = sqlx::query!(
        r#"select id, role, kind, name, phone, inn, merged_into from parties
           where id = any($1) order by id for update"#,
        &all
    )
    .fetch_all(&mut *conn)
    .await?;
    if rows.len() != all.len() {
        return Err(AppError::NotFound);
    }
    let cards: Vec<Card> = rows
        .into_iter()
        .map(|r| Card {
            id: r.id,
            role: r.role,
            kind: r.kind,
            name: r.name,
            phone: r.phone,
            inn: r.inn,
            merged_into: r.merged_into,
        })
        .collect();
    let main = cards
        .iter()
        .find(|c| c.id == id)
        .ok_or(AppError::NotFound)?;
    if cards.iter().any(|c| c.merged_into.is_some()) {
        return Err(AppError::Conflict(
            "одна из карточек уже влита в другую".into(),
        ));
    }
    if cards.iter().any(|c| c.role != main.role) {
        return Err(AppError::Conflict(
            "клиента и поставщика объединить нельзя".into(),
        ));
    }
    let waiting = sqlx::query_scalar!(
        r#"select exists (select 1 from sale_approvals
                          where party_id = any($1) and status = 'pending') as "e!""#,
        &dups
    )
    .fetch_one(&mut *conn)
    .await?;
    if waiting {
        return Err(AppError::Conflict(
            "у карточки есть чек, ждущий подтверждения фирмы — дождитесь ответа".into(),
        ));
    }

    let final_kind = req.kind.clone().unwrap_or_else(|| main.kind.clone());
    let person = person_name(
        &final_kind,
        req.first_name.as_deref(),
        req.last_name.as_deref(),
    )?;
    let new_name = person.as_ref().map(|p| p.2.clone()).or_else(|| {
        req.name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    });
    if new_name.as_deref().is_some_and(|n| n.chars().count() > 200) {
        return Err(invalid("название — не длиннее 200 знаков"));
    }
    let into_name = new_name.clone().unwrap_or_else(|| main.name.clone());
    let is_supplier = main.role == "supplier";

    crate::api::loyalty::lock_party(&mut *conn, id).await?;
    let mut moved_total: i64 = 0;
    let mut from_names: Vec<String> = Vec::with_capacity(dups.len());
    for dup in cards.iter().filter(|c| c.id != id) {
        from_names.push(dup.name.clone());
        let comment_out = format!("Объединение: карточка влита в «{into_name}»");
        let comment_in = format!("Объединение: из карточки «{}»", dup.name);

        // Долг и аванс — по каждому филиалу, где он есть (раздельные долги, задача 036).
        let balances = sqlx::query!(
            "select branch_id, balance_tyiyn from party_balances
             where party_id = $1 and balance_tyiyn <> 0 order by branch_id",
            dup.id
        )
        .fetch_all(&mut *conn)
        .await?;
        let mut moved: i64 = 0;
        for b in &balances {
            for (party, other, amount, comment) in [
                (dup.id, id, -b.balance_tyiyn, &comment_out),
                (id, dup.id, b.balance_tyiyn, &comment_in),
            ] {
                sqlx::query!(
                    r#"insert into party_ledger (id, branch_id, party_id, kind, amount_tyiyn,
                                                 business_date, doc_type, doc_id, comment,
                                                 user_id, device_id)
                       values ($1, $2, $3, 'adjust', $4, (now() at time zone 'Asia/Bishkek')::date,
                               $5, $6, $7, $8, $9)"#,
                    new_id(),
                    b.branch_id,
                    party,
                    amount,
                    MERGE_DOC,
                    other,
                    comment,
                    ctx.user.id,
                    ctx.device_id
                )
                .execute(&mut *conn)
                .await?;
            }
            moved = moved
                .checked_add(b.balance_tyiyn)
                .ok_or_else(|| invalid("слишком большая сумма долга"))?;
        }
        if moved != 0 {
            sqlx::query!(
                "update parties set balance_tyiyn = balance_tyiyn - $2 where id = $1",
                dup.id,
                moved
            )
            .execute(&mut *conn)
            .await?;
            sqlx::query!(
                "update parties set balance_tyiyn = balance_tyiyn + $2 where id = $1",
                id,
                moved
            )
            .execute(&mut *conn)
            .await?;
            moved_total = moved_total
                .checked_add(moved)
                .ok_or_else(|| invalid("слишком большая сумма долга"))?;
        }

        // Баллы — общие на клиента: переходят двумя записями.
        crate::api::loyalty::lock_party(&mut *conn, dup.id).await?;
        let points = sqlx::query_scalar!(
            r#"select coalesce(sum(amount_tyiyn), 0)::bigint as "v!" from loyalty_ledger
               where party_id = $1"#,
            dup.id
        )
        .fetch_one(&mut *conn)
        .await?;
        if points != 0 {
            for (party, other, amount) in [(dup.id, id, -points), (id, dup.id, points)] {
                sqlx::query!(
                    r#"insert into loyalty_ledger (id, branch_id, party_id, kind, amount_tyiyn,
                                                   doc_type, doc_id)
                       values ($1, $2, $3, 'merge', $4, $5, $6)"#,
                    new_id(),
                    ctx.user.branch_id,
                    party,
                    amount,
                    MERGE_DOC,
                    other
                )
                .execute(&mut *conn)
                .await?;
            }
        }

        // Человек, влитый в фирму, становится её работником (агентом): так в чеке видно, кто брал.
        if final_kind == "company" && dup.kind == "person" {
            sqlx::query!(
                r#"insert into party_contacts (id, branch_id, party_id, full_name, phone, inn)
                   values ($1, $2, $3, $4, $5, $6)"#,
                new_id(),
                ctx.user.branch_id,
                id,
                dup.name,
                dup.phone,
                dup.inn
            )
            .execute(&mut *conn)
            .await?;
        }

        // Справочники переходят на основную карточку; проведённое остаётся за влитой (I4).
        sqlx::query!(
            "update party_contacts set party_id = $2 where party_id = $1",
            dup.id,
            id
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query!(
            "update party_vehicles set party_id = $2 where party_id = $1",
            dup.id,
            id
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query!(
            "update party_vehicles set paid_by_party_id = $2 where paid_by_party_id = $1",
            dup.id,
            id
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query!(
            "update telegram_customers set party_id = $2 where party_id = $1",
            dup.id,
            id
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query!(
            "update vehicle_requests set party_id = $2 where party_id = $1",
            dup.id,
            id
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query!(
            "update contact_requests set party_id = $2 where party_id = $1",
            dup.id,
            id
        )
        .execute(&mut *conn)
        .await?;
        // Процент кассира ждёт погашения долга — теперь это долг основной карточки.
        sqlx::query!(
            "update payroll_pending set party_id = $2 where party_id = $1",
            dup.id,
            id
        )
        .execute(&mut *conn)
        .await?;
        // Кабинет: вход влитой карточки переходит, если у основной своего нет; сеансы — заново.
        sqlx::query!("delete from client_sessions where party_id = $1", dup.id)
            .execute(&mut *conn)
            .await?;
        sqlx::query!(
            "update party_accounts set party_id = $2
             where party_id = $1 and not exists (select 1 from party_accounts where party_id = $2)",
            dup.id,
            id
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query!("delete from party_accounts where party_id = $1", dup.id)
            .execute(&mut *conn)
            .await?;
        // Пустой телефон и ИНН основной берутся у влитой.
        sqlx::query!(
            "update parties set phone = case when phone = '' then $2 else phone end,
                                inn = case when inn = '' then $3 else inn end
             where id = $1",
            id,
            dup.phone,
            dup.inn
        )
        .execute(&mut *conn)
        .await?;
        // Карточки, влитые раньше в эту, теперь смотрят на основную.
        sqlx::query!(
            "update parties set merged_into = $2 where merged_into = $1",
            dup.id,
            id
        )
        .execute(&mut *conn)
        .await?;
        sqlx::query!(
            "update parties set active = false, merged_into = $2 where id = $1",
            dup.id,
            id
        )
        .execute(&mut *conn)
        .await?;
        if is_supplier {
            // Справочник поставщиков прихода: влитый больше не выбирается в новой накладной.
            sqlx::query!("update suppliers set active = false where id = $1", dup.id)
                .execute(&mut *conn)
                .await?;
            crate::api::supplier_debts::allocate(&mut *conn, dup.id).await?;
        }
    }

    let clear_person = final_kind == "company";
    sqlx::query!(
        r#"update parties
           set kind = $2,
               name = coalesce($3, name),
               first_name = case when $6 then '' else coalesce($4, first_name) end,
               last_name = case when $6 then '' else coalesce($5, last_name) end
           where id = $1"#,
        id,
        final_kind,
        new_name,
        person.as_ref().map(|p| p.0.clone()),
        person.as_ref().map(|p| p.1.clone()),
        clear_person
    )
    .execute(&mut *conn)
    .await?;
    if is_supplier {
        crate::api::supplier_debts::allocate(&mut *conn, id).await?;
    }

    ops::audit(
        &mut *conn,
        ctx,
        KIND,
        "party",
        Some(id),
        json!({
            "into_name": into_name,
            "from_names": from_names,
            "amount_tyiyn": moved_total,
        }),
    )
    .await?;
    let out = load_party(&mut *conn, ctx.user.branch_id, id).await?;
    ops::finish_op(&mut *conn, ctx, req.op_id, KIND, &out).await?;
    Ok(out)
}
