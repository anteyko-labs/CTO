//! Страницы длинных списков (SPEC-28 §2): `limit` и `offset` в строке запроса, сколько строк во
//! всём отборе — в заголовке ответа `X-Total-Count`. Тело ответа то же, что без страниц, поэтому
//! бот и прежние вызовы не меняются: без `limit` список отдаётся как раньше, до прежнего потолка.

use axum::Json;
use axum::http::{HeaderName, HeaderValue};
use serde::Deserialize;

/// Заголовок с числом строк во всём отборе.
pub const TOTAL_HEADER: HeaderName = HeaderName::from_static("x-total-count");

/// Больше строк одна страница не отдаёт.
pub const MAX_PAGE: i64 = 500;

/// Сколько строк отбора сервер перебирает, чтобы сосчитать их и отдать страницу. Дальше — сузить
/// период или поиск: экран показывает «показаны первые N».
pub const MAX_ROWS: i64 = 10_000;

#[derive(Deserialize, Default, Clone, Copy, Debug)]
pub struct PageQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

impl PageQuery {
    /// Размер страницы: заданный (1…500) или прежний потолок списка `legacy`.
    pub fn size(self, legacy: i64) -> i64 {
        self.limit.map_or(legacy, |l| l.clamp(1, MAX_PAGE))
    }

    pub fn skip(self) -> i64 {
        self.offset.unwrap_or(0).clamp(0, MAX_ROWS)
    }

    /// Страница из уже выбранного отбора и число строк в нём.
    pub fn slice<T>(self, rows: Vec<T>, legacy: i64) -> (Vec<T>, i64) {
        let total = i64::try_from(rows.len()).unwrap_or(i64::MAX);
        let skip = usize::try_from(self.skip()).unwrap_or(0);
        let take = usize::try_from(self.size(legacy)).unwrap_or(0);
        (rows.into_iter().skip(skip).take(take).collect(), total)
    }
}

/// Ответ со страницей: строки в теле, число строк отбора — в заголовке.
pub type Paged<T> = ([(HeaderName, HeaderValue); 1], Json<Vec<T>>);

pub fn paged<T>(items: Vec<T>, total: i64) -> Paged<T> {
    ([(TOTAL_HEADER, HeaderValue::from(total))], Json(items))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slices_pages_and_counts_all_rows() {
        let rows: Vec<i32> = (1..=120).collect();
        let q = PageQuery {
            limit: Some(50),
            offset: Some(100),
        };
        let (page, total) = q.slice(rows.clone(), 500);
        assert_eq!(total, 120);
        assert_eq!(page, (101..=120).collect::<Vec<_>>());
        // Без страниц — как раньше: до прежнего потолка.
        let (page, total) = PageQuery::default().slice(rows.clone(), 100);
        assert_eq!((page.len(), total), (100, 120));
        // Неверные значения не ломают ответ.
        let q = PageQuery {
            limit: Some(0),
            offset: Some(-5),
        };
        assert_eq!(q.slice(rows, 500).0, vec![1]);
    }
}
