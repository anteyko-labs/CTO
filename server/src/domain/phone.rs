//! Номер телефона для сравнения и поиска (ADR-090). Кыргызские номера пишут по-разному:
//! «+996 555 12 34 56», «0555 123 456», «555123456» — это один номер «996555123456».
//! Остальные номера сравниваются всеми цифрами: «+7 955 512-34-56» с кыргызским не совпадёт.
//! То же правило — в функции базы `phone_norm` (миграция 0051).

fn digits(s: &str) -> String {
    s.chars().filter(char::is_ascii_digit).collect()
}

/// Полный номер для сравнения; меньше 9 цифр — не номер.
pub fn key(phone: &str) -> Option<String> {
    let d = digits(phone);
    let full = if d.len() == 12 && d.starts_with("996") {
        d
    } else if d.len() == 10 && d.starts_with('0') {
        format!("996{}", &d[1..])
    } else if d.len() == 9 {
        format!("996{d}")
    } else {
        d
    };
    (full.len() >= 9).then_some(full)
}

/// Цифры для поиска по части номера: «0555 12» → «55512», «+996 555» → «555». Меньше 3 цифр —
/// не ищем по телефону.
pub fn search_digits(q: &str) -> Option<String> {
    let d = digits(q);
    let part = if d.len() >= 12 && d.starts_with("996") {
        d[3..].to_string()
    } else if let Some(rest) = d.strip_prefix('0') {
        rest.to_string()
    } else {
        d
    };
    (part.len() >= 3).then_some(part)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kyrgyz_numbers_match_in_any_form() {
        let k = Some("996555123456".to_string());
        assert_eq!(key("+996 555 12 34 56"), k);
        assert_eq!(key("0555 123 456"), k);
        assert_eq!(key("555123456"), k);
        assert_eq!(key("+7 955 512-34-56"), Some("79555123456".into()));
        assert_ne!(key("+7 955 512-34-56"), k);
        assert_eq!(key("12345"), None);
    }

    #[test]
    fn search_by_part_of_number() {
        assert_eq!(search_digits("0555123456"), Some("555123456".into()));
        assert_eq!(search_digits("+996 555 778 899"), Some("555778899".into()));
        assert_eq!(search_digits("778899"), Some("778899".into()));
        assert_eq!(search_digits("Азамат"), None);
        assert_eq!(search_digits("05"), None);
    }
}
