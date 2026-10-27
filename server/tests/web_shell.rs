//! Оболочка программы всегда свежая: касса получает новую (и откатанную) версию, а не ответ 304
//! по дате файла.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use avtodom_server::{app, build_state};

const FUTURE: &str = "Wed, 01 Jan 2031 00:00:00 GMT";

#[sqlx::test(migrator = "avtodom_server::MIGRATOR")]
async fn shell_ignores_dates_assets_keep_them(pool: PgPool) {
    let dir = std::env::temp_dir().join(format!("avtodom-web-{}", Uuid::now_v7()));
    std::fs::create_dir_all(dir.join("assets")).unwrap();
    std::fs::write(dir.join("index.html"), "<!doctype html>касса").unwrap();
    std::fs::write(dir.join("sw.js"), "self.x = 1").unwrap();
    std::fs::write(dir.join("assets/app-1a2b.js"), "x").unwrap();
    let app = app(build_state(pool, false).await.unwrap(), Some(&dir));
    let get = |uri: &str| {
        Request::get(uri)
            .header(header::IF_MODIFIED_SINCE, FUTURE)
            .body(Body::empty())
            .unwrap()
    };
    // Оболочка: дата в запросе новее файла — всё равно 200 и no-cache (откат версии доходит).
    for uri in ["/sw.js", "/index.html", "/", "/cashier"] {
        let res = app.clone().oneshot(get(uri)).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{uri}");
        assert_eq!(
            res.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-cache",
            "{uri}"
        );
    }
    // Файлы с хэшем в имени по-прежнему отвечают 304.
    let res = app
        .clone()
        .oneshot(get("/assets/app-1a2b.js"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
    // API — без изменений.
    let res = app
        .oneshot(Request::get("/api/v1/nope").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    std::fs::remove_dir_all(&dir).unwrap();
}
