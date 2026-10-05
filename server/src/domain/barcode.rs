//! Собственные штрихкоды EAN-13 с внутримагазинным префиксом (ADR-010).

const PREFIX: &str = "22";

/// Контрольная цифра для 12 цифр EAN-13.
pub fn ean13_check(digits12: &str) -> Option<u8> {
    if digits12.len() != 12 {
        return None;
    }
    let mut sum = 0u32;
    for (i, ch) in digits12.chars().enumerate() {
        let d = ch.to_digit(10)?;
        sum += if i % 2 == 0 { d } else { d * 3 };
    }
    u8::try_from((10 - sum % 10) % 10).ok()
}

/// Внутренний код по порядковому номеру `seq` (1..=9 999 999 999).
pub fn internal_code(seq: i64) -> Option<String> {
    if !(1..=9_999_999_999).contains(&seq) {
        return None;
    }
    let body = format!("{PREFIX}{seq:010}");
    let check = ean13_check(&body)?;
    Some(format!("{body}{check}"))
}

/// Код состоит только из печатных символов и разумной длины.
pub fn is_acceptable(code: &str) -> bool {
    (1..=64).contains(&code.len()) && code.chars().all(|c| c.is_ascii_graphic())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_digit() {
        assert_eq!(ean13_check("220000000001"), Some(9));
        assert_eq!(ean13_check("400638133393"), Some(1));
        assert_eq!(ean13_check("12345"), None);
        assert_eq!(ean13_check("22000000000x"), None);
    }

    #[test]
    fn internal_codes() {
        assert_eq!(internal_code(1).as_deref(), Some("2200000000019"));
        assert_eq!(internal_code(0), None);
    }

    #[test]
    fn acceptable_codes() {
        assert!(is_acceptable("4006381333931"));
        assert!(!is_acceptable(""));
        assert!(!is_acceptable("код"));
        assert!(!is_acceptable("a b"));
    }
}
