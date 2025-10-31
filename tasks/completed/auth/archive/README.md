# Archive

This directory contains superseded authentication design documents that are kept for historical reference.

## Documents

### julien_unified_jwks_architecture_proposal.md

**Status:** SUPERSEDED (2025-01-24)

**Proposed:** Unified JWT validation architecture using local JWKS for service accounts.

**Decision:** After discussion, we decided to use OAuth 2.0 client credentials flow instead of self-signed JWTs with local JWKS. This simplifies the architecture significantly.

**See:** [Service Account Strategy Change](../service_account_strategy_change.md) for the rationale behind this decision.

**Why kept:** Documents the decision-making process and alternative approaches that were considered.
