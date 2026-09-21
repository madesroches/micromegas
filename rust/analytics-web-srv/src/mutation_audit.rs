//! Structured audit record for control-plane mutations (audience grants, groups, and
//! self-service audience claims).
//!
//! Mirrors `QueryAuditRecord` (`micromegas::servers::query_audit`): one JSON-serialized record
//! per mutation attempt, emitted under the dedicated `control_plane_audit` log target, carrying
//! the actor, the action, the target, and the outcome -- allowed *or* denied, so a rejected
//! mutation is no longer invisible to anyone auditing this log. A flat record with optional
//! target fields is used across both grants and groups: the alternative (one record type per
//! module) would put auditing back to matching across several targets, which this exists to fix.
//!
//! Emission happens where the caller is known: in the extractor gate for a pre-handler denial
//! (`GrantGate`/`GroupAdminGate`, in `audience_grants.rs`/`groups.rs`), or in a thin
//! wrapper around each handler otherwise. `IntoResponse` is not the emission point -- it has
//! neither the caller's identity nor `client_ip`, and widening every error variant to carry both
//! would touch far more than emitting at the one or two places the caller is already in hand.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use micromegas::auth::types::AuthContext;
use micromegas::servers::http_utils::get_client_ip;
use micromegas::tracing::prelude::*;
use std::convert::Infallible;

pub const CONTROL_PLANE_AUDIT_TARGET: &str = "control_plane_audit";

/// Max bytes of any caller-controlled string embedded in an audit record. The record is built
/// before `audience_grants.rs`/`groups.rs` validate their request bodies (see the module doc), so
/// this is the only bound on a field before it reaches the log -- matching the precedent set by
/// `MAX_SELECTOR_BYTES` rather than inventing a new budget.
const MAX_AUDIT_FIELD_BYTES: usize = 255;

/// Truncates `value` to at most [`MAX_AUDIT_FIELD_BYTES`] bytes, on a UTF-8 char boundary, and
/// appends `"..."` when truncation happened, so a truncated value in the log is never mistaken
/// for a complete one.
fn truncate_for_audit(value: &str) -> String {
    if value.len() <= MAX_AUDIT_FIELD_BYTES {
        return value.to_string();
    }
    let mut end = MAX_AUDIT_FIELD_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &value[..end])
}

/// Action constants stamped on [`MutationAuditRecord::action`]. `&'static str`, not an enum, so
/// the record can be serialized without a custom `Serialize` impl and so a new call site can
/// reuse an existing constant without matching on it.
pub mod action {
    pub const CREATE_GRANT: &str = "create_grant";
    pub const DELETE_GRANT: &str = "delete_grant";
    pub const CLAIM_AUDIENCE: &str = "claim_audience";
    pub const CREATE_GROUP: &str = "create_group";
    pub const DELETE_GROUP: &str = "delete_group";
    pub const ADD_MEMBER: &str = "add_member";
    pub const REMOVE_MEMBER: &str = "remove_member";
}

/// One control-plane mutation attempt, allowed or denied. Flat rather than nested, so every
/// field is one `jsonb_get` away from a `log_entries` query. `audience`/`axis`/`selector` (the
/// grant shape) and `group`/`member` (the group shape) are disjoint in practice -- a given
/// record populates one set or the other, never both -- and both are absent on a gate-level
/// denial, since the body/path hasn't parsed yet at that point.
#[derive(serde::Serialize)]
pub struct MutationAuditRecord {
    /// Caller email, else subject, else `"unauthenticated"` when no identity was available at
    /// all (a missing `AuthContext`/`ValidatedUser` extension).
    pub actor: String,
    pub is_admin: bool,
    pub action: &'static str,
    /// `"allowed"` | `"denied"` | `"error"`.
    pub outcome: &'static str,
    pub client_ip: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audience: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub axis: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
    /// Only on an `allowed` idempotent create: `false` when the row already existed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<bool>,
    /// The denial/error message. Absent when `outcome == "allowed"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Classifies an error variant for the audit record. Implemented next to each error enum this
/// crate already has (`AudienceGrantError`/`GroupsError`/`IngestionKeyError`), rather than
/// matched on here, so a new error variant opts in at its own definition site instead of via an
/// edit to this module.
pub trait AuditOutcome {
    /// `("denied", Some(msg))` for a caller-attributable refusal -- authorization or validation
    /// alike -- `("error", Some(msg))` for a server-side failure the caller didn't cause.
    fn audit_outcome(&self) -> (&'static str, Option<String>);
}

/// Builds one [`MutationAuditRecord`] and emits it exactly once via [`Self::emit`] or
/// [`Self::emit_gate_outcome`]. Built as a handler's (or gate's) first statement, before the
/// fallible work it is auditing runs, so a new early return can't silently skip the emission.
pub struct MutationAudit {
    record: MutationAuditRecord,
}

impl MutationAudit {
    pub fn new(action: &'static str, actor: String, is_admin: bool, client_ip: String) -> Self {
        Self {
            record: MutationAuditRecord {
                actor,
                is_admin,
                action,
                outcome: "allowed",
                client_ip,
                audience: None,
                axis: None,
                selector: None,
                group: None,
                member: None,
                created: None,
                reason: None,
            },
        }
    }

    /// For the `AuthContext`-bearing grant/claim paths. `actor` resolves email, else subject --
    /// the same resolution every mint/revoke/import handler in this crate already uses.
    pub fn from_context(action: &'static str, caller: &AuthContext, client_ip: &ClientIp) -> Self {
        let actor = caller
            .email
            .clone()
            .unwrap_or_else(|| caller.subject.clone());
        Self::new(action, actor, caller.is_admin(), client_ip.0.clone())
    }

    pub fn audience(mut self, audience: &str) -> Self {
        self.record.audience = Some(truncate_for_audit(audience));
        self
    }

    pub fn axis(mut self, axis: &str) -> Self {
        self.record.axis = Some(truncate_for_audit(axis));
        self
    }

    pub fn selector(mut self, selector: &str) -> Self {
        self.record.selector = Some(truncate_for_audit(selector));
        self
    }

    /// Convenience for the grant shape, which always sets all three together.
    pub fn grant(self, audience: &str, axis: &str, selector: &str) -> Self {
        self.audience(audience).axis(axis).selector(selector)
    }

    pub fn group(mut self, group: &str) -> Self {
        self.record.group = Some(truncate_for_audit(group));
        self
    }

    pub fn member(mut self, member: &str) -> Self {
        self.record.member = Some(truncate_for_audit(member));
        self
    }

    pub fn created(mut self, created: bool) -> Self {
        self.record.created = Some(created);
        self
    }

    /// Classifies `result` -- `Ok` as `"allowed"`, `Err` via [`AuditOutcome::audit_outcome`] --
    /// and emits one record. Takes `result` by reference so a caller can still return it
    /// afterward without needing to reconstruct it.
    pub fn emit<T, E: AuditOutcome>(mut self, result: &Result<T, E>) {
        match result {
            Ok(_) => {
                self.record.outcome = "allowed";
                self.record.reason = None;
            }
            Err(e) => {
                let (outcome, reason) = e.audit_outcome();
                self.record.outcome = outcome;
                self.record.reason = reason.map(|r| truncate_for_audit(&r));
            }
        }
        self.emit_record();
    }

    /// For a gate rejection, which has no `Result` to classify: `outcome` is `"denied"` or
    /// `"error"`, chosen by the caller directly.
    pub fn emit_gate_outcome(mut self, outcome: &'static str, reason: impl Into<String>) {
        self.record.outcome = outcome;
        self.record.reason = Some(truncate_for_audit(&reason.into()));
        self.emit_record();
    }

    fn emit_record(&self) {
        match serde_json::to_string(&self.record) {
            Ok(json) => info!(target: CONTROL_PLANE_AUDIT_TARGET, "{json}"),
            Err(e) => warn!("failed to serialize control-plane audit record: {e}"),
        }
    }
}

/// The caller's network-level address, resolved the same way `QueryAuditRecord`'s `client_ip`
/// is: the rightmost `X-Forwarded-For` entry, then `X-Real-IP`, then the socket address --
/// `unknown` if none is available. Infallible, so it can be added to any handler's argument list
/// with no new error path; declared before any body extractor (`Json`/`Bytes`/...) in a
/// handler's signature, per axum's `FromRequestParts`-then-`FromRequest` ordering.
pub struct ClientIp(pub String);

impl<S: Send + Sync> FromRequestParts<S> for ClientIp {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(ClientIp(get_client_ip(&parts.headers, &parts.extensions)))
    }
}
