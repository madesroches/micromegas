# Ingestion Mint "Access Denied" Hint Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1544

## Overview

A non-admin who can self-service-mint an ingestion API key (per `MintPolicy`/`MICROMEGAS_SELF_SERVICE_MINT`) already has a fully working mint screen at `/audiences` (Audience Access). But a non-admin who instead lands on the admin-only `/admin/ingestion-keys` page — an old bookmark, a link an admin pasted, or just the obvious guess from the "Ingestion API Keys" admin card — hits a dead-end "Access Denied" screen with no pointer to where the same capability actually lives. This plan closes that specific reachability gap by adding an optional hint to the denial screen, rather than duplicating the mint UI onto a second, non-admin-safe page.

## Current State

**Backend already treats minting as non-admin-reachable, correctly.** `mint_key` (`rust/analytics-web-srv/src/ingestion_keys.rs:328-492`) is gated by `MintGate`/`AuthenticatedUser`, not `AdminUser`: a non-admin caller may mint once `MICROMEGAS_SELF_SERVICE_MINT` is on and `MintPolicy::resolve_audience` finds a matching `mint` grant (or the request names a fresh audience, lazily claiming it). `list_keys` (`ingestion_keys.rs:827-877`), `revoke_key` (`ingestion_keys.rs:894-922`), and `import_key` (`ingestion_keys.rs:974-1047`) stay `AdminUser`-gated — exactly the split the issue's own notes describe as correct.

**The frontend already has a working non-admin mint screen: Audience Access (`/audiences`).**
- Registered outside `/admin` in the router (`router.tsx:51`), so `AppShell`'s `isAdminRoute` sidebar gate (`AppShell.tsx:13-14`) never applies to it.
- Linked from the header user-menu to every authenticated user, no admin check (`Header.tsx:166-173`).
- `AudienceAccessPage.tsx` wraps its content in `<AuthGuard>` with no `requireAdmin` (line 778).
- Calls `fetchMyAudiences()` (`lib/audience-grants-api.ts:126-147`, backed by `GET /api/audience-grants/my-audiences`) to learn `is_admin`, `audiences`, `mint_prefix`, `held_pairs`; a 403 (self-service off for a non-admin) is treated as "hide the Mint button," not an error (`AudienceAccessPage.tsx:573-595`, `showMintButton` at line 757).
- `MintKeyDialog` (`AudienceAccessPage.tsx:324-525`) calls `mintIngestionApiKey` directly — the same call the admin page's mint dialog makes.
- This is already the documented design: `mkdocs/docs/admin/api-keys.md:382-387` calls `/audiences` "the self-service counterpart of these two admin-only pages."

**What stays gated, correctly.** `/admin` (`AdminPage.tsx:12`, `<AuthGuard requireAdmin>`) and `/admin/ingestion-keys` (`IngestionApiKeysPage.tsx:47`, `ApiKeysAdminPage.tsx:164`, both `<AuthGuard requireAdmin>`) — because that page's list/revoke controls (`ApiKeysAdminPage.tsx`, the keys table and its revoke button) must stay admin-only, and `Sidebar.tsx:308`/`AppShell.tsx:13-14` correctly hide the admin hub and nav entry from non-admins.

**The actual gap**: `AuthGuard`'s denial screen (`AuthGuard.tsx:88-104`) is generic — "This page requires admin access," full stop. For every other `requireAdmin` page (Data Sources, Maps, Query Deny List, the admin hub itself) that's correct: there is no non-admin alternative to point to. `/admin/ingestion-keys` is the one page where there *is* a real alternative, and the denial screen doesn't mention it.

## Design

Add an optional hint to `AuthGuard`'s denial screen, threaded through only for the ingestion-keys admin page.

- **`components/AuthGuard.tsx`**: add `deniedHint?: React.ReactNode` to `AuthGuardProps`. Render it, only when provided, directly under the existing "This page requires admin access." paragraph in the `requireAdmin && !user?.is_admin` branch. Every other call site (unchanged callers pass no prop) keeps today's exact output.
- **`components/ApiKeysAdminPage.tsx`**: add `deniedHint?: React.ReactNode` to `ApiKeysAdminPageConfig`; pass `config.deniedHint` into the existing `<AuthGuard requireAdmin>` at line 164.
- **`routes/IngestionApiKeysPage.tsx`**: set `deniedHint` on `ingestionApiKeysPageConfig` to a short message with an `AppLink` to `/audiences` (e.g. "Mint your own ingestion key from Audience Access instead."), and pass the same node to the page's own `Suspense`-fallback `<AuthGuard requireAdmin>` (line 47) for consistency. `AnalyticsApiKeysPage.tsx`'s config is left unset — analytics-key minting has no self-service counterpart, so there is nothing accurate to point a denied non-admin at.
- No change to `Sidebar.tsx`, `AppShell.tsx`, `AdminPage.tsx`, or any backend route — this is a client-side denial-screen fix, not a new access path.

## Trade-offs

- **Duplicate the mint UI onto `/admin/ingestion-keys` for non-admins, gating list/revoke separately** — rejected. It would mean either lifting `MintKeyDialog`/`fetchMyAudiences` wiring out of `AudienceAccessPage.tsx` into a shared component, or duplicating ~200 lines of already-shipped, already-tested UI, plus loosening `AppShell`'s `isAdminRoute` prefix check for one specific `/admin/*` path. `/audiences` already does this correctly and is already the documented self-service counterpart; a second copy has no functional upside.
- **Redirect `/admin/ingestion-keys` to `/audiences` for a non-admin** — rejected. A silent `useEffect` navigate away from a URL the user deliberately typed or clicked is more surprising than an explanatory denial with a link, and it would special-case one `requireAdmin` page's redirect behavior against every other one.
- **No code change, close as already-resolved** — rejected. The dead-end for a non-admin who lands on the URL directly is real and cheap to fix; a doc-only note doesn't help someone already staring at the Access Denied screen.

## Implementation Steps

1. `analytics-web-app/src/components/AuthGuard.tsx`: add `deniedHint?: React.ReactNode` to `AuthGuardProps`; render it under the existing denial message when present.
2. `analytics-web-app/src/components/ApiKeysAdminPage.tsx`: add `deniedHint?: React.ReactNode` to `ApiKeysAdminPageConfig`; pass `config.deniedHint` into `<AuthGuard requireAdmin>`.
3. `analytics-web-app/src/routes/IngestionApiKeysPage.tsx`: set `deniedHint` (an `AppLink` to `/audiences`) on `ingestionApiKeysPageConfig` and on the `Suspense` fallback's `<AuthGuard requireAdmin>`.
4. Tests: extend `AuthGuard.test.tsx` with a denied-state case asserting `deniedHint` renders when passed and is absent when omitted; extend `IngestionApiKeysPage.test.tsx` with a non-admin case (mirroring the admin/non-admin split in `AudienceAccessPage.test.tsx`) asserting the denial screen links to `/audiences`.
5. Docs: update `mkdocs/docs/admin/api-keys.md`'s "Web app admin pages" section (~line 382-387) to note the admin ingestion-keys page's denial screen now links a non-admin to Audience Access.
6. `CHANGELOG.md`: add an entry under `## Unreleased` / **Web App**.

## Files to Modify

- `analytics-web-app/src/components/AuthGuard.tsx`
- `analytics-web-app/src/components/ApiKeysAdminPage.tsx`
- `analytics-web-app/src/routes/IngestionApiKeysPage.tsx`
- `analytics-web-app/src/components/__tests__/AuthGuard.test.tsx`
- `analytics-web-app/src/routes/__tests__/IngestionApiKeysPage.test.tsx`
- `mkdocs/docs/admin/api-keys.md`
- `CHANGELOG.md`

## Documentation

`mkdocs/docs/admin/api-keys.md`, "Web app admin pages" section.

## Testing Strategy

- `yarn test` (Vitest) — new `AuthGuard` prop coverage, and a non-admin case on `IngestionApiKeysPage.test.tsx` asserting the hint/link render.
- `yarn lint && yarn type-check` from `analytics-web-app/`.
- Manual: run the monolith, sign in as a non-admin, navigate directly to `/admin/ingestion-keys`, confirm the Access Denied screen shows the hint and the link lands on a working `/audiences` mint dialog.

## Open Questions

None blocking.
