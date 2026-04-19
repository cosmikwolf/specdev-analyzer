//! Small string helpers used by audit.

/// Convert `CamelCase` / `PascalCase` to `snake_case`.
pub fn snake_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    let chars: Vec<char> = s.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if c.is_uppercase() {
            let prev_lower = i > 0 && chars[i - 1].is_lowercase();
            let prev_upper = i > 0 && chars[i - 1].is_uppercase();
            let next_lower = i + 1 < chars.len() && chars[i + 1].is_lowercase();
            if i > 0 && (prev_lower || (prev_upper && next_lower)) {
                out.push('_');
            }
            for lc in c.to_lowercase() {
                out.push(lc);
            }
        } else {
            out.push(*c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_cases() {
        assert_eq!(snake_case("StatusLed"), "status_led");
        assert_eq!(snake_case("BtA2dpSink"), "bt_a2dp_sink");
        assert_eq!(snake_case("already_snake"), "already_snake");
        assert_eq!(snake_case("A"), "a");
        assert_eq!(snake_case("HTMLParser"), "html_parser");
    }
}
