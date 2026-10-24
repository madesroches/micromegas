//! Trimming of `file!()` paths before they leave the process.

fn is_sep(b: u8) -> bool {
    b == b'/' || b == b'\\'
}

fn is_absolute(path: &str) -> bool {
    let b = path.as_bytes();
    match b {
        [first, ..] if is_sep(*first) => true,
        [drive, b':', sep, ..] => drive.is_ascii_alphabetic() && is_sep(*sep),
        _ => false,
    }
}

/// Index just past the first `first<sep>second<sep>` sequence that starts at a component boundary.
fn after_marker(path: &str, first: &str, second: &str) -> Option<usize> {
    let b = path.as_bytes();
    let (f, s) = (first.as_bytes(), second.as_bytes());
    let len = f.len() + 1 + s.len() + 1;
    (0..b.len()).find_map(|i| {
        let boundary = i == 0 || is_sep(b[i - 1]);
        let rest = &b[i..];
        (boundary
            && rest.len() >= len
            && rest.starts_with(f)
            && is_sep(rest[f.len()])
            && rest[f.len() + 1..].starts_with(s)
            && is_sep(rest[len - 1]))
        .then_some(i + len)
    })
}

fn file_name(path: &str) -> &str {
    match path.bytes().rposition(is_sep) {
        Some(i) => &path[i + 1..],
        None => path,
    }
}

/// Returns the part of a `file!()` path worth sending: a suffix of `path`, so no allocation.
///
/// Relative paths (workspace members) are unchanged. Absolute paths into the cargo registry lose
/// everything through the index directory, git checkouts lose everything through
/// `git/checkouts/`, and any other absolute path is reduced to its file name. Both `/` and `\`
/// separate components, because the path comes from the build host, which may differ from the
/// target.
pub fn trim_source_path(path: &str) -> &str {
    if !is_absolute(path) {
        return path;
    }
    if let Some(start) = after_marker(path, "registry", "src") {
        let rest = &path[start..];
        if let Some(i) = rest.bytes().position(is_sep) {
            let trimmed = &rest[i + 1..];
            if !trimmed.is_empty() {
                return trimmed;
            }
        }
    }
    if let Some(start) = after_marker(path, "git", "checkouts") {
        let trimmed = &path[start..];
        if !trimmed.is_empty() {
            return trimmed;
        }
    }
    file_name(path)
}
