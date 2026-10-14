//! Госномер в одном виде (SPEC-22): «01 kg-123 АВС» и «01KG123ABC» — один номер.
//! То же правило в базе — функция `plate_norm` (миграция 0028), сверка в тестах.

/// Кириллица, похожая на латиницу, — в латиницу (заглавные и строчные).
fn latin(c: char) -> char {
    match c {
        'А' | 'а' => 'A',
        'В' | 'в' => 'B',
        'Е' | 'е' => 'E',
        'К' | 'к' => 'K',
        'М' | 'м' => 'M',
        'Н' | 'н' => 'H',
        'О' | 'о' => 'O',
        'Р' | 'р' => 'P',
        'С' | 'с' => 'C',
        'Т' | 'т' => 'T',
        'У' | 'у' => 'Y',
        'Х' | 'х' => 'X',
        other => other.to_ascii_uppercase(),
    }
}

/// Номер для сравнения: без пробелов и дефисов, латиница заглавными.
/// Остальные знаки не трогаются — так же, как в базе.
pub fn normalize(plate: &str) -> String {
    plate
        .chars()
        .filter(|c| !matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{a0}' | '-'))
        .map(latin)
        .collect()
}

/// Номер, который клиент ввёл в боте: 4–12 латинских букв и цифр после приведения.
pub fn parse(input: &str) -> Option<String> {
    let n = normalize(input.trim());
    let len = n.chars().count();
    ((4..=12).contains(&len) && n.chars().all(|c| c.is_ascii_alphanumeric())).then_some(n)
}

/// Образцы для сверки с функцией базы.
pub const SAMPLES: [&str; 10] = [
    "01 kg 123 abc",
    "01KG123ABC",
    "01 КG 123 АВС",
    "о777ко-01",
    "B\u{a0}123-XY",
    "a-b c\td",
    "Б123ЖЖ",
    "01kg123abc",
    "мнорстух",
    "",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_spaces_case_and_cyrillic() {
        assert_eq!(normalize("01 kg 123 abc"), "01KG123ABC");
        assert_eq!(normalize("01 КG 123 АВС"), "01KG123ABC");
        assert_eq!(normalize("о777ко-01"), "O777KO01");
        assert_eq!(normalize("Б123ЖЖ"), "Б123ЖЖ");
    }

    #[test]
    fn parse_checks_length_and_letters() {
        assert_eq!(parse(" 01 kg 123 abc ").as_deref(), Some("01KG123ABC"));
        assert_eq!(parse("01 KG 123 АВС").as_deref(), Some("01KG123ABC"));
        assert_eq!(parse("A12"), None);
        assert_eq!(parse("1234567890123"), None);
        assert_eq!(parse("Б123ЖЖ"), None);
        assert_eq!(parse("01KG.123"), None);
        assert_eq!(parse("A123").as_deref(), Some("A123"));
    }
}
