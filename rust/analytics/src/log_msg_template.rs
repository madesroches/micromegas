use micromegas_transit::value::{Object, Value};

/// Renders a log message from the template the instrumented process deferred: `fmt_str` with
/// each `{name}` or `{name:?}` replaced by the string property `name`.
///
/// A missing property, or a placeholder outside that grammar, is copied through unchanged, so a
/// corrupt or hand-built block stays readable instead of failing.
pub fn render_msg_template(fmt_str: &str, properties: &Object) -> String {
    let mut out = String::with_capacity(fmt_str.len());
    let mut rest = fmt_str;
    while let Some(pos) = rest.find(['{', '}']) {
        out.push_str(&rest[..pos]);
        let tail = &rest[pos..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
            continue;
        }
        if tail.starts_with('{')
            && let Some(end) = tail.find('}')
        {
            let placeholder = &tail[..=end];
            let inner = &tail[1..end];
            // `{n:}` is `{n}` in Rust's format grammar and reaches here verbatim.
            let name = inner
                .strip_suffix(":?")
                .or_else(|| inner.strip_suffix(':'))
                .unwrap_or(inner);
            match properties.members.iter().find(|(k, _)| *k == name) {
                Some((_, Value::String(value))) => out.push_str(value),
                _ => out.push_str(placeholder),
            }
            rest = &tail[end + 1..];
            continue;
        }
        out.push_str(&tail[..1]);
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}
