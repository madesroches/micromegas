//! Sanitizing of caller-controlled text before it reaches a log line, and the shared
//! auth-rejection log line.

use crate::types::RequestParts;
use micromegas_tracing::prelude::*;

fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Makes `value` safe to put in a log line: control and bidi override/isolate characters become
/// `?`, so one line can't pretend to be two or render reordered in a terminal. Longer than
/// `max_chars` characters is cut to `max_chars - 1` characters plus `…`.
pub fn sanitize_for_log(value: &str, max_chars: usize) -> String {
    let truncate = value.chars().nth(max_chars).is_some();
    let kept = if truncate {
        max_chars.saturating_sub(1)
    } else {
        max_chars
    };
    let mut out: String = value
        .chars()
        .take(kept)
        .map(|c| {
            if c.is_control() || is_bidi_control(c) {
                '?'
            } else {
                c
            }
        })
        .collect();
    if truncate && max_chars > 0 {
        out.push('…');
    }
    out
}

/// Logs an authentication rejection with the caller's network details as properties.
///
/// Only the request path (never the query string) and the `user-agent` header are read, so a
/// credential has no way into the line.
pub fn warn_auth_rejection(prefix: &'static str, parts: &dyn RequestParts, err: &anyhow::Error) {
    let client_ip = parts
        .client_ip()
        .map(|ip| ip.to_string())
        .unwrap_or_default();
    let method = sanitize_for_log(parts.method().unwrap_or_default(), 16);
    let path = sanitize_for_log(parts.uri().unwrap_or_default(), 256);
    let user_agent = sanitize_for_log(parts.get_header("user-agent").unwrap_or_default(), 256);
    let reason = sanitize_for_log(&format!("{err}"), 1024);
    warn!(
        client_ip = %client_ip,
        method = %method,
        path = %path,
        user_agent = %user_agent,
        "{} {}",
        prefix,
        reason
    );
}
