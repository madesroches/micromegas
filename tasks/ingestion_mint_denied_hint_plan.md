# Non-Admin Ingestion Mint Access Plan

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1544

## Overview

A non-admin who can self-service-mint an ingestion API key (per `MintPolicy`/`MICROMEGAS_SELF_SERVICE_MINT`) can already do so on the Audience Access page (`/audiences`), but the Admin hub (`/admin`) and the Ingestion API Keys page (`/admin/ingestion-keys`) are both hard-gated to admins, so a non-admin who navigates there directly — an old bookmark, a link an admin pasted, or the natural place to look for "Ingestion API Keys" — hits a dead end. This plan makes those two pages viewable by every authenticated user, with their content filtered by role: a non-admin sees a reduced Admin hub (only the cards they have some capability in) and a reduced Ingestion API Keys page (mint only, no list/revoke table), reusing the self-service mint flow already proven out on `/audiences` rather than building a second copy of it.

## Current State

**Backend already treats minting as non-admin-reachable, correctly.** `mint_key` (`rust/analytics-web-srv/src/ingestion_keys.rs:328-492`) is gated by `MintGate`/`AuthenticatedUser`, not `AdminUser`: a non-admin caller may mint once `MICROMEGAS_SELF_SERVICE_MINT` is on and `MintPolicy::resolve_audience` finds a matching `mint` grant (or the request names a fresh audience, lazily claiming it). `list_keys` (`ingestion_keys.rs:827-877`), `revoke_key` (`ingestion_keys.rs:894-922`), and `import_key` (`ingestion_keys.rs:974-1047`) stay `AdminUser`-gated — exactly the split the issue's own notes describe as correct, and this plan makes no backend change.

**The frontend already has a working non-admin mint flow, on `/audiences` only.**
- `AudienceAccessPage.tsx` (route registered at `router.tsx:51`, outside `/admin`) wraps its content in `<AuthGuard>` with no `requireAdmin` (line 778), and is linked from the header user-menu to every authenticated user (`Header.tsx:166-173`).
- It calls `fetchMyAudiences()` (`lib/audience-grants-api.ts:126-147`, backed by `GET /api/audience-grants/my-audiences`) to learn `is_admin`, `audiences`, `mint_prefix`, `held_pairs`; a 403 (self-service off for a non-admin) is treated as "hide the Mint button," not an error (`AudienceAccessPage.tsx:573-595`, `showMintButton` at line 757).
- `MintKeyDialog` (`AudienceAccessPage.tsx:324-525`), defined locally in that file, drives the audience picker (a held audience, or "New audience…" which lazily claims one) and calls `mintIngestionApiKey`.

**What's still hard-gated today:**
- `/admin` (`AdminPage.tsx:12`, `<AuthGuard requireAdmin>`) — the hub page listing all eight admin cards (Data Sources, Export/Import Screens, Maps, Ingestion API Keys, Analytics API Keys, Query Deny List, Audience Access).
- `/admin/ingestion-keys` (`IngestionApiKeysPage.tsx:47`, `ApiKeysAdminPage.tsx:164`, both `<AuthGuard requireAdmin>`) — the admin key-management page, whose list/mint/revoke table is `ApiKeysAdminPage.tsx`, shared with `/admin/analytics-keys` via an `ApiKeysAdminPageConfig` object (`ApiKeysAdminPage.tsx:34-55`).
- `Sidebar.tsx:308` hides the bottom-left "Admin" nav icon entirely unless `user?.is_admin`.
- `AppShell.tsx:10-14` hides the whole `Sidebar` for any `/admin*` route unless `user?.is_admin`, via one blanket `isAdminRoute = pathname.startsWith('/admin')` check.

Every one of these is a legitimate gate for the six cards/pages that have no non-admin capability at all (Data Sources, Export/Import Screens, Maps, Analytics API Keys — no self-service mint counterpart — and Query Deny List). It is not legitimate for `/admin` itself or `/admin/ingestion-keys`, both of which now have a real non-admin capability (viewing the filtered hub; minting) that the current all-or-nothing gate hides along with the admin-only parts.

## Design

Make `/admin` and `/admin/ingestion-keys` viewable by every authenticated user, filtering each page's content by `user?.is_admin`, and extract the mint-dialog machinery `/audiences` already built so both pages share one implementation instead of two.

**1. Extract the self-service mint pieces out of `AudienceAccessPage.tsx` so `/admin/ingestion-keys` can reuse them:**
- `hooks/useMyAudiences.ts` (new): the `me`/`selfServiceOff`/`authDisabled`/`myAudiencesError` state and `loadMyAudiences` callback currently inlined at `AudienceAccessPage.tsx:538-546,573-595`, returning `{ me, selfServiceOff, authDisabled, error, reload }`. `AudienceAccessPage.tsx` switches to this hook; behavior is unchanged (same fetch, same 403/`AUTH_DISABLED`/generic-error handling).
- `components/MintIngestionKeyDialog.tsx` (new): `AudienceAccessPage.tsx`'s `MintKeyDialog` function (lines 324-525), moved verbatim and exported. `AudienceAccessPage.tsx` imports it instead of defining it locally.

**2. `IngestionApiKeysPage.tsx` becomes role-aware**, replacing its current `<Suspense fallback={<AuthGuard requireAdmin>...}><ApiKeysAdminPage .../></Suspense>` body:
- Wrap everything in one `<AuthGuard>` (no `requireAdmin`) so loading/unauthenticated states behave as they do on every non-admin-gated page.
- Inside, branch on `user?.is_admin`: an admin renders `<ApiKeysAdminPage config={ingestionApiKeysPageConfig} pageSize={pageSize} />` exactly as today (that component's own internal `<AuthGuard requireAdmin>` at `ApiKeysAdminPage.tsx:164` becomes a redundant-but-harmless inner check, unchanged, and still the only gate for `/admin/analytics-keys`, which is untouched by this plan).
- A non-admin renders a new `IngestionKeysSelfServicePanel` component: `useMyAudiences()` for `me`, a "Mint Key" button opening `MintIngestionKeyDialog`, and — when `selfServiceOff` — the same explanatory copy pattern `/audiences` uses ("Self-service is disabled on this deployment... ask an admin"). No table, no revoke UI: those routes stay `AdminUser`-gated server-side, so there is nothing this panel could show for them. Include a line pointing to `/audiences` for reviewing held grants. The panel calls the hook's `reload()` in a mount effect, mirroring `AudienceAccessPage.tsx`'s own mount effect that calls `loadMyAudiences()` — without it, `me` stays `null` forever and the Mint button never appears. Its `onMinted` handler also calls `reload()` when `response.claimed` is true, mirroring `AudienceAccessPage.tsx`'s `handleMinted`, so a freshly claimed audience shows up for a subsequent mint.

**3. `AdminPage.tsx` drops `requireAdmin` and filters its card grid by role:**
- Change `<AuthGuard requireAdmin>` (both the content wrapper at line 12 and the `Suspense` fallback at line 129) to plain `<AuthGuard>`.
- Turn the eight `AppLink` card blocks into a data array (`{ href, icon, title, description, adminOnly }`), filtered by `isAdmin || !card.adminOnly` before rendering, instead of one large fixed JSX block. `adminOnly: true` for Data Sources, Export Screens, Import Screens, Maps, Analytics API Keys, Query Deny List; `adminOnly: false` for Ingestion API Keys and Audience Access.
- The Ingestion API Keys card's description becomes role-aware ("Mint, list, and revoke write credentials..." for an admin; "Mint your own write credentials for telemetry ingestion clients." for a non-admin), since list/revoke don't apply to what a non-admin actually gets on that page.
- The page subtitle becomes role-aware too ("System administration and data management tools." for an admin; "Tools you have access to." for a non-admin).

**4. `AppShell.tsx`'s sidebar gate narrows from "the whole `/admin` prefix" to the subset that's still fully admin-gated:**
```ts
const ADMIN_ONLY_PATHS = [
  '/admin/data-sources',
  '/admin/export-screens',
  '/admin/import-screens',
  '/admin/maps',
  '/admin/analytics-keys',
  '/admin/query-deny-list',
]
const isAdminOnlyRoute = ADMIN_ONLY_PATHS.some((p) => pathname.startsWith(p))
const showSidebar = status === 'authenticated' && (!isAdminOnlyRoute || user?.is_admin)
```
`/admin` and `/admin/ingestion-keys` are no longer in the blocked set, so the sidebar now shows for a non-admin on those two routes, same as on `/audiences` today.

**5. `Sidebar.tsx`'s "Admin" nav icon shows to every authenticated user**, not just `user?.is_admin` (`Sidebar.tsx:308`) — it now points at a hub that renders something for everyone.

No backend change, and no change to `/admin/data-sources`, `/admin/export-screens`, `/admin/import-screens`, `/admin/maps`, `/admin/analytics-keys`, or `/admin/query-deny-list` — those keep `<AuthGuard requireAdmin>` exactly as today, both directly and via `AppShell`'s narrowed gate.

## Trade-offs

- **Add the self-service mint branch straight into the shared `ApiKeysAdminPage`/`ApiKeysAdminPageConfig`**, rather than branching in `IngestionApiKeysPage.tsx` before reaching it — rejected. `ApiKeysAdminPage` is also used unmodified by `/admin/analytics-keys`, which has no self-service story at all; threading a role branch through the shared component and its config would force every future config field to reason about a case that only one of its two consumers needs. Branching in `IngestionApiKeysPage.tsx` keeps `ApiKeysAdminPage.tsx` and `AnalyticsApiKeysPage.tsx` completely untouched.
- **Duplicate `MintKeyDialog`/`fetchMyAudiences` onto the ingestion-keys page instead of extracting them** — rejected as a DRY violation: ~200 lines of already-shipped, already-tested mint UI would exist twice and drift. Extracting `useMyAudiences`/`MintIngestionKeyDialog` costs one refactor of `AudienceAccessPage.tsx` (behavior-preserving) in exchange for one shared implementation.
- **Redirect `/admin`/`/admin/ingestion-keys` to `/audiences` for a non-admin** instead of rendering filtered content in place — rejected: a non-admin following an admin-shared "check the ingestion keys page" link should land on that page, not be bounced somewhere else; role-filtering in place matches how every other multi-role page in this app (`/audiences` itself) already works.

## Decisions

- User directed the admin screen to be viewable by all with role-filtered content, superseding this plan's original "Access-Denied hint linking to `/audiences`" approach.

## Implementation Steps

1. `analytics-web-app/src/hooks/useMyAudiences.ts` (new): extract the my-audiences load state/callback from `AudienceAccessPage.tsx`.
2. `analytics-web-app/src/components/MintIngestionKeyDialog.tsx` (new): extract `MintKeyDialog` from `AudienceAccessPage.tsx`, exported.
3. `analytics-web-app/src/routes/AudienceAccessPage.tsx`: switch to the new hook and component; verify behavior and existing tests are unaffected.
4. `analytics-web-app/src/routes/IngestionApiKeysPage.tsx`: single outer `<AuthGuard>`, branch admin (`<ApiKeysAdminPage>`, unchanged) vs. non-admin (new `IngestionKeysSelfServicePanel`, using the extracted hook/dialog). The panel calls the hook's `reload()` in a mount effect (mirroring `AudienceAccessPage.tsx`'s mount effect), and calls `reload()` from `onMinted` when `response.claimed` is true (mirroring `AudienceAccessPage.tsx`'s `handleMinted`).
5. `analytics-web-app/src/routes/AdminPage.tsx`: drop `requireAdmin`; convert the card grid to a filtered, role-aware data array; role-aware subtitle.
6. `analytics-web-app/src/components/layout/AppShell.tsx`: replace the blanket `/admin` prefix check with the explicit `ADMIN_ONLY_PATHS` list.
7. `analytics-web-app/src/components/layout/Sidebar.tsx`: show the Admin nav icon unconditionally for an authenticated user.
8. Tests:
   - `AppShell.test.tsx`: add cases asserting the sidebar renders for a non-admin on `/admin` and `/admin/ingestion-keys`, and still doesn't on `/admin/maps` (existing case, unchanged).
   - New `AdminPage.test.tsx`: admin sees all eight cards; non-admin sees only Ingestion API Keys and Audience Access.
   - `IngestionApiKeysPage.test.tsx`: add a non-admin describe block covering the self-service panel (mint flow, `selfServiceOff` copy, no table/revoke UI), mirroring `AudienceAccessPage.test.tsx`'s admin/non-admin split.
   - `AudienceAccessPage.test.tsx`: confirm it still passes unmodified against the extracted hook/component (adjust only if its mocks reach into the file's internals rather than rendered output).
9. Docs: update `mkdocs/docs/admin/web-app.md` (Admin hub section) and `mkdocs/docs/admin/api-keys.md`'s "Web app admin pages" section to describe `/admin` and `/admin/ingestion-keys` as viewable-by-all with role-filtered content, distinct from the still-fully-admin-gated pages.
10. `CHANGELOG.md`: add an entry under `## Unreleased` / **Web App**.

## Files to Modify

- `analytics-web-app/src/hooks/useMyAudiences.ts` (new)
- `analytics-web-app/src/components/MintIngestionKeyDialog.tsx` (new)
- `analytics-web-app/src/routes/AudienceAccessPage.tsx`
- `analytics-web-app/src/routes/IngestionApiKeysPage.tsx`
- `analytics-web-app/src/routes/AdminPage.tsx`
- `analytics-web-app/src/components/layout/AppShell.tsx`
- `analytics-web-app/src/components/layout/Sidebar.tsx`
- `analytics-web-app/src/components/layout/__tests__/AppShell.test.tsx`
- `analytics-web-app/src/routes/__tests__/AdminPage.test.tsx` (new)
- `analytics-web-app/src/routes/__tests__/IngestionApiKeysPage.test.tsx`
- `analytics-web-app/src/routes/__tests__/AudienceAccessPage.test.tsx`
- `mkdocs/docs/admin/web-app.md`
- `mkdocs/docs/admin/api-keys.md`
- `CHANGELOG.md`

## Documentation

`mkdocs/docs/admin/web-app.md` (Admin hub) and `mkdocs/docs/admin/api-keys.md` ("Web app admin pages" section).

## Testing Strategy

- `yarn test` (Vitest) — `AppShell`, `AdminPage`, `IngestionApiKeysPage`, and `AudienceAccessPage` suites.
- `yarn lint && yarn type-check` from `analytics-web-app/`.
- Manual: run the monolith, sign in as a non-admin with a mint grant, confirm `/admin` shows only the two filtered cards, `/admin/ingestion-keys` shows a working mint dialog with no table, and `/admin/data-sources` (still admin-only) still denies. Repeat as an admin and confirm nothing changed there.

## Open Questions

None blocking.
