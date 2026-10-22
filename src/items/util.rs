//! Small string helpers used by item extraction.

/// `path` relative to `root` when it lies under it, else unchanged: what
/// the item commands print and write, so no output names a home directory.
pub fn relative_to(root: &std::path::Path, path: &std::path::Path) -> std::path::PathBuf {
    path.strip_prefix(root)
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|_| path.to_path_buf())
}

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

/// The file stems a module name may map to, most conventional first, with
/// no duplicates: [`snake_case`] (`Aced2Format` → `aced2format`,
/// `SysEx7` → `sys_ex7`); the same with a word break after a digit, as
/// heck and rustc's `non_snake_case` lint split it (`aced2_format`); and
/// the name lowercased with no breaks (`sysex7`).
pub fn snake_case_variants(s: &str) -> Vec<String> {
    let plain = snake_case(s);
    let chars: Vec<char> = s.chars().collect();
    let mut digit_break = String::with_capacity(s.len() + 4);
    for (i, c) in chars.iter().enumerate() {
        if c.is_uppercase() {
            let prev = (i > 0).then(|| chars[i - 1]);
            let prev_lower = prev.is_some_and(|p| p.is_lowercase());
            let prev_digit = prev.is_some_and(|p| p.is_ascii_digit());
            let prev_upper = prev.is_some_and(|p| p.is_uppercase());
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            if i > 0 && (prev_lower || prev_digit || (prev_upper && next_lower)) {
                digit_break.push('_');
            }
            digit_break.extend(c.to_lowercase());
        } else {
            digit_break.push(*c);
        }
    }
    let joined: String = s
        .chars()
        .filter(|c| *c != '_')
        .flat_map(char::to_lowercase)
        .collect();
    let mut out = vec![plain];
    for v in [digit_break, joined] {
        if !out.contains(&v) {
            out.push(v);
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

    #[test]
    fn variants_cover_digit_names() {
        assert_eq!(
            snake_case_variants("Aced2Format"),
            ["aced2format", "aced2_format"]
        );
        assert_eq!(snake_case_variants("SysEx7"), ["sys_ex7", "sysex7"]);
        assert_eq!(
            snake_case_variants("StatusLed"),
            ["status_led", "statusled"]
        );
        assert_eq!(snake_case_variants("clock"), ["clock"]);
    }
}
