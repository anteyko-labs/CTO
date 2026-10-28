//! Пароли, сессии и извлечение текущего пользователя (SPEC-01).

use argon2::password_hash::SaltString;
use argon2::password_hash::rand_core::OsRng;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{AppError, AppResult, invalid};
use crate::state::AppState;

pub const SESSION_COOKIE: &str = "avtodom_session";
pub const SESSION_DAYS: i64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    Admin,
}

impl Role {
    pub fn parse(s: &str) -> AppResult<Self> {
        match s {
            "owner" => Ok(Self::Owner),
            "admin" => Ok(Self::Admin),
            _ => Err(invalid("неизвестная роль")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CurrentUser {
    pub id: Uuid,
    pub branch_id: Uuid,
    pub login: String,
    pub full_name: String,
    pub role: Role,
}

impl CurrentUser {
    /// Тот же пользователь в другом филиале — для отчёта «Все филиалы» владельца (SPEC-33 §3).
    pub fn in_branch(&self, branch_id: Uuid) -> Self {
        let mut u = self.clone();
        u.branch_id = branch_id;
        u
    }

    pub fn is_owner(&self) -> bool {
        self.role == Role::Owner
    }

    pub fn require_owner(&self) -> AppResult<()> {
        if self.is_owner() {
            Ok(())
        } else {
            Err(AppError::Forbidden)
        }
    }
}

/// Право администратора (SPEC-30 §5, ADR-097): ключ, подпись (экран «Что можно» и ответ 403),
/// группа на экране и выдаётся ли оно новому администратору.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct PermissionDef {
    pub key: &'static str,
    pub label: &'static str,
    pub group: &'static str,
    pub default: bool,
}

const fn perm(
    key: &'static str,
    label: &'static str,
    group: &'static str,
    default: bool,
) -> PermissionDef {
    PermissionDef {
        key,
        label,
        group,
        default,
    }
}

/// Права администратора — весь список на сервере в одном месте (на экране — `web/src/lib/permissions.ts`).
/// Владельцу права не нужны: он может всё. Правом не выдаются себестоимость, прибыль, маржа
/// (инвариант 6), «Исправить» документов (ADR-085), цены товаров (ADR-032), отмена перелива,
/// начисления, бонусы и удержания — это только владелец (ADR-097).
pub const PERMISSIONS: [PermissionDef; 18] = [
    perm("sale_return", "Возврат по чеку", "Касса", true),
    perm("sale_debt", "Продажа в долг", "Касса", true),
    perm("discount", "Скидка в чеке", "Касса", false),
    perm("debt_payment", "Оплата долга клиента", "Касса", true),
    perm(
        "cash_moves",
        "Внести, изъять, перевести деньги",
        "Деньги",
        true,
    ),
    perm("expenses", "Расходы", "Деньги", true),
    perm("payroll_pay", "Выплаты мастерам", "Деньги", true),
    perm("payroll_advance", "Аванс мастерам", "Деньги", false),
    perm(
        "receipts",
        "Приход товара и оплата поставщику",
        "Склад",
        true,
    ),
    perm("stock_in", "Оприходование", "Склад", true),
    perm("supplier_returns", "Возврат поставщику", "Склад", true),
    perm("defects", "Отметка брака", "Склад", true),
    perm("revision", "Ревизия", "Склад", true),
    perm("oil_transfer", "Перелив масла", "Склад", false),
    perm("battery_intake", "Приём аккумуляторов", "Склад", true),
    // Планшет приёмки (SPEC-39 §8): без подготовленной накладной, PIN и подписи кассира ничего
    // не проводит, поэтому «да» по умолчанию.
    perm("acceptance", "Приёмка товара (планшет)", "Склад", true),
    perm("products", "Товары и штрихкоды", "Справочники", true),
    perm("clients", "Клиенты и машины", "Справочники", true),
];

/// Описание права по ключу; неизвестное — `None`.
pub fn permission_def(key: &str) -> Option<&'static PermissionDef> {
    PERMISSIONS.iter().find(|p| p.key == key)
}

/// Права, которые получает новый администратор (SPEC-30 §5).
pub fn default_permissions() -> Vec<&'static str> {
    PERMISSIONS
        .iter()
        .filter(|p| p.default)
        .map(|p| p.key)
        .collect()
}

/// Ответ администратору без права: 403 с названием права — «Нет права: Расходы» (ADR-097).
pub fn no_permission(key: &str) -> AppError {
    AppError::ForbiddenWith(format!(
        "Нет права: {}",
        permission_def(key).map_or(key, |p| p.label)
    ))
}

/// Владелец проходит всегда; администратор — если владелец выдал ему право, иначе 403
/// «Нет права: …» (ADR-097).
pub async fn require_permission(
    conn: &mut sqlx::PgConnection,
    user: &CurrentUser,
    permission: &str,
) -> AppResult<()> {
    if has_permission(conn, user, permission).await? {
        Ok(())
    } else {
        Err(no_permission(permission))
    }
}

/// Права по умолчанию новому администратору (SPEC-30 §5); уже выданные не трогаются.
/// `granted_by` — кто завёл администратора.
pub async fn grant_default_permissions(
    conn: &mut sqlx::PgConnection,
    user_id: Uuid,
    branch_id: Uuid,
    granted_by: Uuid,
) -> AppResult<()> {
    let keys: Vec<String> = default_permissions()
        .into_iter()
        .map(str::to_string)
        .collect();
    sqlx::query!(
        r#"insert into user_permissions (user_id, branch_id, permission, granted_by)
           select $1, $2, p, $4 from unnest($3::text[]) as p
           on conflict do nothing"#,
        user_id,
        branch_id,
        &keys,
        granted_by
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Права пользователя сверх роли — экран показывает по ним пункты меню, проверяет сервер.
pub async fn permissions(conn: &mut sqlx::PgConnection, user_id: Uuid) -> AppResult<Vec<String>> {
    Ok(sqlx::query_scalar!(
        "select permission from user_permissions where user_id = $1 order by permission",
        user_id
    )
    .fetch_all(&mut *conn)
    .await?)
}

/// Владельцу можно всё; администратору — если владелец выдал ему это право (ADR-082, ADR-097).
pub async fn has_permission(
    conn: &mut sqlx::PgConnection,
    user: &CurrentUser,
    permission: &str,
) -> AppResult<bool> {
    if user.is_owner() {
        return Ok(true);
    }
    Ok(sqlx::query_scalar!(
        r#"select exists (select 1 from user_permissions
                          where user_id = $1 and branch_id = $2 and permission = $3) as "e!""#,
        user.id,
        user.branch_id,
        permission
    )
    .fetch_one(&mut *conn)
    .await?)
}

/// Пользователь и устройство изменяющего запроса.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub user: CurrentUser,
    pub device_id: Uuid,
    /// Операция пришла из чата владельца в Телеграме (SPEC-24): журнал помечает источник «бот»
    /// и чат. `None` — программа.
    pub bot_chat: Option<i64>,
}

/// Устройство операций, проведённых владельцем через Телеграм-бот (SPEC-24): одно на всех,
/// чтобы в журнале и документах было видно, что это не касса и не телефон владельца.
pub const TELEGRAM_DEVICE: Uuid = Uuid::from_u128(0x7e1e_6a3d_0000_4000_8000_0000_0000_0b07);

impl Ctx {
    /// Тот же пользователь и устройство, а запись — в другом филиале (SPEC-33 §2): часть
    /// погашения, закрывающая долг другого филиала, ложится на филиал долга.
    pub fn in_branch(&self, branch_id: Uuid) -> Self {
        Self {
            user: self.user.in_branch(branch_id),
            device_id: self.device_id,
            bot_chat: self.bot_chat,
        }
    }

    /// Владелец, проводящий операцию кнопкой в чате бота.
    pub fn bot(user: CurrentUser, chat_id: i64) -> Self {
        Self {
            user,
            device_id: TELEGRAM_DEVICE,
            bot_chat: Some(chat_id),
        }
    }
}

/// Одновременных проверок argon2: каждая берёт около 19 МБ памяти и ядро процессора. Поток
/// неверных входов ждёт своей очереди, а не съедает память сервера точки (ADR-128).
const ARGON_PERMITS: usize = 3;
/// Ждущих проверки пароля входа больше этого — сразу 429, без хеширования.
pub const LOGIN_QUEUE: usize = 32;
static ARGON: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(ARGON_PERMITS);
static LOGIN_WAITING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Место в очереди проверки пароля входа; уходит вместе с запросом, даже оборванным.
struct LoginSlot;

impl Drop for LoginSlot {
    fn drop(&mut self) {
        LOGIN_WAITING.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

pub async fn hash_password(password: String) -> AppResult<String> {
    let _permit = ARGON
        .acquire()
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?;
    tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| AppError::Internal(e.to_string()))
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?
}

/// Проверка пароля при входе (сотрудник, кабинет юрлица): очередь длиннее `LOGIN_QUEUE` —
/// 429 сразу, без хеширования (ADR-128).
pub async fn verify_login_password(password: String, hash: String) -> AppResult<bool> {
    if LOGIN_WAITING.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= LOGIN_QUEUE {
        LOGIN_WAITING.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        return Err(AppError::TooManyRequests(
            "сервер занят проверкой входов — повторите через минуту".into(),
        ));
    }
    let _slot = LoginSlot;
    Ok(verify_password(password, hash).await)
}

pub async fn verify_password(password: String, hash: String) -> bool {
    let Ok(_permit) = ARGON.acquire().await else {
        return false;
    };
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&hash)
            .map(|parsed| {
                Argon2::default()
                    .verify_password(password.as_bytes(), &parsed)
                    .is_ok()
            })
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}

pub fn new_token() -> (String, Vec<u8>) {
    let raw: [u8; 32] = rand::random();
    let token = hex::encode(raw);
    let hash = token_hash(&token);
    (token, hash)
}

pub fn token_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

pub fn cookie_value<'a>(parts: &'a Parts, name: &str) -> Option<&'a str> {
    parts
        .headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v)
}

pub fn session_token(parts: &Parts) -> Option<&str> {
    cookie_value(parts, SESSION_COOKIE)
}

/// Адрес клиента для предела попыток входа (ADR-092). Сервер слушает 127.0.0.1, к нему ходят
/// наши прокси на той же машине: Caddy (входящий X-Forwarded-For заменяет своим) и
/// `tailscale serve` (дописывает адрес в конец цепочки). Поэтому адрес — последний в цепочке:
/// первые подставляет кто угодно. Пришли напрямую не с локального адреса и не с прокси из
/// `TRUSTED_PROXIES` (через запятую, например адрес Caddy в Docker) — заголовкам не верим,
/// берём адрес соединения. Заголовок Cloudflare — только за туннелем (TRUST_CF_HEADER=1) и только
/// если он совпадает с последним адресом X-Forwarded-For или того нет: Caddy кассы и
/// `tailscale serve` тоже ходят с 127.0.0.1 и чужой CF-Connecting-IP не убирают (ADR-128).
pub fn client_addr(parts: &Parts) -> String {
    let peer = parts
        .extensions
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map(|c| c.0.ip());
    let trusted = std::env::var("TRUSTED_PROXIES").unwrap_or_default();
    let cf = std::env::var("TRUST_CF_HEADER").is_ok_and(|v| v == "1");
    client_addr_with(parts, peer, &trusted, cf)
}

fn client_addr_with(
    parts: &Parts,
    peer: Option<std::net::IpAddr>,
    trusted: &str,
    cf: bool,
) -> String {
    let via_proxy = peer.is_none_or(|ip| {
        ip.is_loopback()
            || trusted
                .split(',')
                .filter_map(|t| t.trim().parse::<std::net::IpAddr>().ok())
                .any(|t| t == ip)
    });
    if !via_proxy {
        return peer.map_or_else(|| "local".into(), |ip| ip.to_string());
    }
    let header = |name: &str| {
        parts
            .headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.rsplit(',').map(str::trim).find(|x| !x.is_empty()))
            .filter(|v| v.len() <= 64)
            .map(str::to_string)
    };
    let xff = header("x-forwarded-for");
    cf.then(|| header("cf-connecting-ip"))
        .flatten()
        .filter(|c| xff.as_ref().is_none_or(|x| x == c))
        .or(xff)
        .or_else(|| header("x-real-ip"))
        .or_else(|| peer.map(|ip| ip.to_string()))
        .unwrap_or_else(|| "local".into())
}

pub fn device_id(parts: &Parts) -> Option<Uuid> {
    parts
        .headers
        .get("x-device-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| Uuid::parse_str(v).ok())
}

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = session_token(parts).ok_or(AppError::Unauthorized)?;
        let hash = token_hash(token);
        let row = sqlx::query!(
            r#"select u.id, u.branch_id, u.login, u.full_name, u.role
               from sessions s join users u on u.id = s.user_id
               where s.token_hash = $1 and s.expires_at > now() and u.active"#,
            hash
        )
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::Unauthorized)?;
        let role = Role::parse(&row.role)?;
        let branch_id = request_branch(parts, state, role, row.branch_id).await?;
        Ok(Self {
            id: row.id,
            branch_id,
            login: row.login,
            full_name: row.full_name,
            role,
        })
    }
}

/// Заголовок выбранного филиала (SPEC-33 §1): владелец шлёт его с каждым запросом, выбор помнит
/// устройство. У администратора заголовок не читается — он всегда в своём филиале.
pub const BRANCH_HEADER: &str = "x-branch-id";

/// Филиал запроса (SPEC-33, ADR-108). Администратор — только свой филиал (`users.branch_id`),
/// что бы ни прислал. Владелец — филиал из заголовка `X-Branch-Id`, без заголовка — свой;
/// неизвестный филиал — 400, чтобы действие не ушло в другой филиал молча. В закрытом филиале
/// изменения запрещены (история остаётся); открыть его снова можно в «Настройки → Филиалы».
async fn request_branch(
    parts: &Parts,
    state: &AppState,
    role: Role,
    home: Uuid,
) -> AppResult<Uuid> {
    let picked = match parts.headers.get(BRANCH_HEADER) {
        Some(v) if role == Role::Owner => Some(
            v.to_str()
                .ok()
                .and_then(|v| Uuid::parse_str(v.trim()).ok())
                .ok_or_else(|| invalid("филиал не найден — выберите филиал"))?,
        ),
        _ => None,
    };
    let branch_id = picked.unwrap_or(home);
    let archived = sqlx::query_scalar!("select archived from branches where id = $1", branch_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(|| invalid("филиал не найден — выберите филиал"))?;
    let read = matches!(
        parts.method,
        axum::http::Method::GET | axum::http::Method::HEAD
    );
    let path = parts.uri.path();
    let exempt = path.starts_with("/branches") || path.starts_with("/auth/");
    if archived && !read && !exempt {
        return Err(AppError::Conflict(
            "филиал закрыт: изменения в нём недоступны".into(),
        ));
    }
    Ok(branch_id)
}

impl FromRequestParts<AppState> for Ctx {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user = CurrentUser::from_request_parts(parts, state).await?;
        let device_id = device_id(parts).ok_or_else(|| invalid("нужен заголовок X-Device-Id"))?;
        Ok(Self {
            user,
            device_id,
            bot_chat: None,
        })
    }
}

/// Владелец, иначе 403.
pub struct Owner(pub CurrentUser);

impl FromRequestParts<AppState> for Owner {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user = CurrentUser::from_request_parts(parts, state).await?;
        user.require_owner()?;
        Ok(Self(user))
    }
}

#[cfg(test)]
mod tests {
    use super::{PERMISSIONS, client_addr_with, default_permissions};
    use axum::http::Request;

    #[test]
    fn permissions_match_the_screen_list() {
        // Список прав (SPEC-30 §5) — на сервере и на экране; ключ, подпись и «по умолчанию»
        // должны совпадать, иначе «Нет права: …» и флажок назовут право по-разному.
        assert_eq!(PERMISSIONS.len(), 18);
        assert_eq!(default_permissions().len(), 15);
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../web/src/lib/permissions.ts");
        let Ok(ts) = std::fs::read_to_string(path) else {
            // Сборка одного сервера без клиента: сверять не с чем.
            return;
        };
        for p in PERMISSIONS {
            let at = ts
                .find(&format!("key: '{}'", p.key))
                .unwrap_or_else(|| panic!("на экране нет права {}", p.key));
            let entry = &ts[at..at + ts[at..].find('}').unwrap()];
            assert!(entry.contains(&format!("label: '{}'", p.label)), "{entry}");
            assert!(entry.contains(&format!("group: '{}'", p.group)), "{entry}");
            assert!(
                entry.contains(&format!("default: {}", p.default)),
                "{entry}"
            );
        }
    }

    fn parts(xff: Option<&str>) -> axum::http::request::Parts {
        let mut b = Request::builder().uri("/");
        if let Some(v) = xff {
            b = b.header("x-forwarded-for", v);
        }
        b.body(()).unwrap().into_parts().0
    }

    #[test]
    fn login_limit_key_is_not_set_by_the_client() {
        let local = Some("127.0.0.1".parse().unwrap());
        // Через tailscale serve: подставленный клиентом адрес первый, настоящий — последний.
        assert_eq!(
            client_addr_with(&parts(Some("6.6.6.6, 100.64.0.7")), local, "", false),
            "100.64.0.7"
        );
        assert_eq!(
            client_addr_with(&parts(Some("192.168.1.20")), local, "", false),
            "192.168.1.20"
        );
        // Напрямую с чужого адреса — заголовку не верим.
        let lan = Some("192.168.1.30".parse().unwrap());
        assert_eq!(
            client_addr_with(&parts(Some("6.6.6.6")), lan, "", false),
            "192.168.1.30"
        );
        // Прокси из TRUSTED_PROXIES (Caddy в Docker) — адрес из заголовка.
        assert_eq!(
            client_addr_with(
                &parts(Some("10.0.0.5")),
                Some("172.18.0.2".parse().unwrap()),
                "172.18.0.2",
                false
            ),
            "10.0.0.5"
        );
        assert_eq!(
            client_addr_with(&parts(None), local, "", false),
            "127.0.0.1"
        );
        assert_eq!(client_addr_with(&parts(None), None, "", false), "local");
    }

    fn with_cf(xff: Option<&str>, cf: &str) -> axum::http::request::Parts {
        let mut p = parts(xff);
        p.headers.insert("cf-connecting-ip", cf.parse().unwrap());
        p
    }

    #[test]
    fn cf_header_only_from_the_tunnel() {
        let local = Some("127.0.0.1".parse().unwrap());
        // Туннель Cloudflare: заголовок и X-Forwarded-For — один адрес.
        assert_eq!(
            client_addr_with(&with_cf(Some("5.5.5.5"), "5.5.5.5"), local, "", true),
            "5.5.5.5"
        );
        assert_eq!(
            client_addr_with(&with_cf(None, "5.5.5.5"), local, "", true),
            "5.5.5.5"
        );
        // Касса через Caddy с подставленным заголовком: верим адресу от Caddy.
        for fake in ["10.0.2.1", "10.0.2.2", "10.0.2.3"] {
            assert_eq!(
                client_addr_with(&with_cf(Some("192.168.1.20"), fake), local, "", true),
                "192.168.1.20"
            );
        }
        // Без TRUST_CF_HEADER заголовок не читается.
        assert_eq!(
            client_addr_with(&with_cf(Some("192.168.1.20"), "5.5.5.5"), local, "", false),
            "192.168.1.20"
        );
    }
}
