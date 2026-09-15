use similar::TextDiff;

pub fn unified_diff(path: &str, old: Option<&str>, new: &str) -> String {
    let diff = TextDiff::from_lines(old.unwrap_or(""), new);
    let (a, b) = match old {
        Some(_) => (format!("a/{path}"), format!("b/{path}")),
        None => ("/dev/null".to_string(), format!("b/{path}")),
    };
    let out = diff
        .unified_diff()
        .context_radius(3)
        .header(&a, &b)
        .to_string();
    let out = out.trim().to_string();
    if out.is_empty() {
        format!("no changes: {path}")
    } else {
        cap(&out, 8192)
    }
}

pub fn cap(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let half = max / 2;
    let head: String = s.chars().take(half).collect();
    let tail: String = s.chars().skip(s.chars().count() - half).collect();
    format!("{head}\n... [truncated] ...\n{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_contains_plus_minus() {
        let d = unified_diff("notes.txt", Some("alpha\nbeta\n"), "alpha\ngamma\n");
        assert!(d.contains("-beta"), "{d}");
        assert!(d.contains("+gamma"), "{d}");
    }

    #[test]
    fn diff_new_file_all_adds() {
        let d = unified_diff("new.txt", None, "hello\nworld\n");
        assert!(d.contains("+hello"), "{d}");
        assert!(d.contains("+world"), "{d}");
    }
}
