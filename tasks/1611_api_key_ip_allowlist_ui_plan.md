# API-Key IP Allowlist Web UI Plan (#1611)

**GitHub Issue**: https://github.com/madesroches/micromegas/issues/1611

## Overview

Show API-key IP allowlists in the analytics web app. #1600 added the backend: the
`allowed_cidrs` column, the optional `allowed_cidrs` mint parameter, and the admin-only
`PATCH .../{table}-api-keys/{key_id}/allowlist` route. This plan adds the browser side: an
**IP Allowlist** column in both admin key tables, an admin edit action that calls the `PATCH`
route, and an optional allowlist field in both mint dialogs. An empty allowlist, which every
existing key has, means "no restriction". The column shows that as *Unrestricted*, not as a
blank cell.

## Current State

### Backend (done in #1600)

- `rust/analytics-web-srv/src/ingestion_keys.rs` / `analytics_keys.rs`:
  - `MintRequest.allowed_cidrs: Option<Vec<String>>` (`ingestion_keys.rs:299`, `analytics_keys.rs:154`).
  - List rows always carry `allowed_cidrs: Vec<String>`. The server reads it as
    `COALESCE(allowed_cidrs, '{}')`, so the field is never absent or `null`.
  - `PATCH {base_path}/api/{ingestion,analytics}-api-keys/{key_id}/allowlist` takes the body
    `{"allowed_cidrs": [...]}` and returns 200 `{"allowed_cidrs": [...]}`, or 404. It is
    `AdminUser`-gated (`analytics_keys.rs:351-384`, `ingestion_keys.rs:884+`). `[]` clears the
    restriction.
  - Both mint and `PATCH` validate with `IpAllowlist::parse` (`rust/auth/src/ip_allowlist.rs`).
    A bad entry returns 400 `{code, message}`, where the message looks like
    `invalid allowed_cidrs: invalid CIDR or IP address: "foo"` or
    `... has host bits set; did you mean "10.0.0.0/8"?`.
- The REST contract is documented in `mkdocs/docs/admin/api-keys.md:106-113` and in its
  `## IP allowlisting` section.

### Web app (nothing yet)

- `analytics-web-app/src/lib/api-keys-shared.ts` holds the factory both key clients use.
  `ApiKeyListEntry` (`:11-21`) has no allowlist field. `mint(name, audience?)` (`:113-123`)
  posts `{name, audience}`. There is no `PATCH` function.
- `lib/ingestion-api-keys-api.ts` and `lib/analytics-api-keys-api.ts` are thin wrappers that
  re-export `api.list/mint/revoke`.
- `components/ApiKeysAdminPage.tsx` is the shared admin list/mint/revoke page, configured by
  `AnalyticsApiKeysPage.tsx` and by the admin branch of `IngestionApiKeysPage.tsx`.
  - It has its **own inline mint form** (`:195-269`), not `MintIngestionKeyDialog`. So an admin
    minting on either page never sees that dialog, and both mint surfaces need the field.
  - The table columns are Name / Created / Last Used / [Audience] / Status / Actions
    (`:329-398`). The only row action is Revoke, and it is hidden on revoked keys.
  - After each mutation the page calls `loadKeys()` to refetch the list.
- `components/MintIngestionKeyDialog.tsx` is the self-service mint dialog. It is used by
  `AudienceAccessPage.tsx:686` and by the non-admin panel of `IngestionApiKeysPage.tsx`
  (`IngestionKeysSelfServicePanel`). It calls `mintIngestionApiKey(name, audience)` directly and
  shows `err.message` on failure.
- The allowlist is edited through the `PATCH` route, which is admin-only. `ApiKeysAdminPage`
  only renders for admins (`<AuthGuard requireAdmin>` on the analytics page; the `is_admin`
  branch on the ingestion page), so every surface that shows the edit action is one where the
  route is callable. Non-admins can set an allowlist at mint time but can't change it later.

## Design

### Client library — `lib/api-keys-shared.ts`

```ts
export interface ApiKeyListEntry {
  // ...existing fields
  /** Empty = unrestricted. The server COALESCEs NULL to [], so this is always present. */
  allowed_cidrs: string[]
}

export interface MintApiKeyOptions {
  audience?: string
  allowed_cidrs?: string[]
}

export interface SetAllowlistResponse {
  allowed_cidrs: string[]
}

export interface ApiKeysApi<TRevokeResponse> {
  ErrorClass: ApiKeyErrorConstructor
  list: (...) => Promise<ApiKeyListEntry[]>
  mint: (name: string, options?: MintApiKeyOptions) => Promise<MintApiKeyResponse>
  revoke: (keyId: string) => Promise<TRevokeResponse>
  setAllowlist: (keyId: string, allowedCidrs: string[]) => Promise<SetAllowlistResponse>
}

/** Splits free-text input on newlines, commas, and whitespace, and drops blanks. No validation:
 *  the server's `IpAllowlist::parse` is the one validator, and its 400 message is shown to the
 *  user unchanged. */
export function parseAllowlistInput(text: string): string[]
```

- `mint` becomes `mint(name, options = {})`. The body is
  `JSON.stringify({ name, audience: options.audience, allowed_cidrs: options.allowed_cidrs })`.
  `JSON.stringify` drops `undefined` keys, so callers that don't pass an allowlist send the same
  bytes as today. Moving `audience` into an options object is a deliberate break. Every current
  call passes a string as the second argument, so the TypeScript compiler flags each call site
  that needs updating. Without this, analytics callers would have to write
  `mint(name, undefined, cidrs)`.
- `setAllowlist` sends `PATCH ${basePath}/${encodeURIComponent(keyId)}/allowlist` with body
  `{allowed_cidrs}` and reuses `handleResponse`.
- The thin modules add `export const setIngestionApiKeyAllowlist = api.setAllowlist` and
  `export const setAnalyticsApiKeyAllowlist = api.setAllowlist`.

### Shared input — `components/AllowedCidrsField.tsx` (new)

A controlled `<textarea>` (monospace, `rows={3}`) with a label ("Allowed IPs / CIDR ranges")
and fixed help text: *"One entry per line (commas also work). Bare IPs match that single
address. Leave empty to allow this key from any IP."* The props are `{ value: string; onChange:
(v: string) => void; optional?: boolean }`. `optional` adds a muted "(optional)" suffix to the
label; the mint dialogs set it and the edit dialog doesn't. All three surfaces render this one
component, so the wording and styling stay in one place. The styling copies the input classes
already used in `ApiKeysAdminPage`/`MintIngestionKeyDialog`.

Each dialog keeps the raw text in state and calls `parseAllowlistInput` only on submit. Mint
sends `allowed_cidrs: parsed.length ? parsed : undefined`, which omits the field and matches
today's request. Edit always sends the parsed array, so an empty textarea sends `[]` and clears
the restriction.

### Edit dialog — `components/EditAllowlistDialog.tsx` (new)

```ts
export function EditAllowlistDialog(props: {
  keyName: string
  initialCidrs: string[]
  onSave: (allowedCidrs: string[]) => Promise<unknown>
  onClose: () => void
})
```

- It uses the same modal markup as the inline mint form (backdrop, `max-w-md` panel, header,
  body, Cancel/Save footer). The title is `IP allowlist — {keyName}`.
- The textarea starts as `initialCidrs.join('\n')`. The parent mounts the dialog only while it
  has a target and passes `key={target.key_id}`. The `useState` initializer therefore handles the
  prefill, and no reset-on-open effect is needed (compare the `wasOpenRef` machinery in
  `MintIngestionKeyDialog`).
- Save awaits `onSave(parseAllowlistInput(text))`.
  - On failure, the dialog stays open and shows the error message in the same error box the mint
    form uses. This is how the server's validation message reaches the user.
  - On success, the parent closes the dialog and calls `loadKeys()`. The parent refetches rather
    than patching the row locally from the response.
  - The backdrop and Cancel are disabled while a save is in flight.

### Admin page — `components/ApiKeysAdminPage.tsx`

- `ApiKeysAdminPageConfig` changes:
  - `mintKey: (name: string, options?: MintApiKeyOptions) => Promise<MintApiKeyResponse>`.
  - New **required** field `setAllowlist: (keyId: string, allowedCidrs: string[]) =>
    Promise<unknown>`. Both pages have the route, so the field isn't optional, and making it
    required means the compiler rejects any config that leaves it out.
- **Column**: add `IP Allowlist` between Audience and Status. It is unconditional, since both key
  tables have allowlists.
  - Non-empty: one `font-mono text-xs` line per entry. Allowlists are short in practice, so there
    is no truncation or "+N more".
  - Empty: `<span className="text-theme-text-muted italic">Unrestricted</span>`.
  - Revoked keys still show their allowlist, for the audit trail.
- **Row action**: add a `Shield` (lucide) icon button before Revoke, with `title="Edit IP
  allowlist"` and `aria-label={`Edit IP allowlist for ${key.name}`}`. Like Revoke, it is shown
  only when `!key.revoked_at`. A revoked key can't authenticate, so its allowlist has no effect.
  Clicking it sets `allowlistTarget` state, which mounts `EditAllowlistDialog`.
- **Inline mint form**: add `<AllowedCidrsField optional />` below Name/Audience. Add a
  `mintAllowlist` state, reset in `openMintForm`. `handleMint` calls
  `config.mintKey(name, { audience, allowed_cidrs })`.

### Page configs

- `AnalyticsApiKeysPage.tsx`: `setAllowlist: setAnalyticsApiKeyAllowlist`.
- `IngestionApiKeysPage.tsx`: `setAllowlist: setIngestionApiKeyAllowlist`.
- `AudienceAccessPage.tsx`: no change. It gets the new field through `MintIngestionKeyDialog`.

### Self-service mint — `components/MintIngestionKeyDialog.tsx`

Add `<AllowedCidrsField optional />` below the Audience block. Add an `allowlistText` state and
clear it in the existing `justOpened` reset branch. Call
`mintIngestionApiKey(name, { audience: resolvedAudience || undefined, allowed_cidrs })`. The
field doesn't affect whether Mint is enabled. A bad entry comes back as a 400 whose message the
dialog already shows.

## Mockups

- `tasks/1611_api_key_ip_allowlist_ui_mockups/allowlist-column-and-dialogs.html` has four
  panels: (1) the admin list with the new column (restricted, unrestricted, and revoked rows) and
  the shield edit action, (2) the edit dialog prefilled, (3) the edit dialog showing a server 400
  message, and (4) the mint dialog with the optional field. It is a single option. The change is
  small and follows the existing table/modal conventions, so there was no real layout choice to
  compare.

## Implementation Steps

1. **Client lib** (`lib/api-keys-shared.ts`): add `allowed_cidrs` to `ApiKeyListEntry`, add
   `MintApiKeyOptions`, and change `mint` to take options. Add `SetAllowlistResponse`,
   `setAllowlist`, and `parseAllowlistInput`.
2. **Thin modules** (`lib/ingestion-api-keys-api.ts`, `lib/analytics-api-keys-api.ts`): export
   the `set*ApiKeyAllowlist` functions.
3. **`components/AllowedCidrsField.tsx`** (new).
4. **`components/EditAllowlistDialog.tsx`** (new).
5. **`components/ApiKeysAdminPage.tsx`**: config type changes, the column, the row action and
   dialog mount, and the mint-form field.
6. **Page configs**: `routes/AnalyticsApiKeysPage.tsx` and `routes/IngestionApiKeysPage.tsx`
   gain `setAllowlist`.
7. **`components/MintIngestionKeyDialog.tsx`**: the field and the options-object mint call.
8. Add `allowed_cidrs: []` to every list-response fixture in
   `routes/__tests__/{Ingestion,Analytics}ApiKeysPage.test.tsx`, including both `makeKeys`
   helpers — these fixtures are untyped plain JSON, so `yarn type-check` won't catch a missing
   field, but the page reads `key.allowed_cidrs.length` at runtime. Then fix whatever `yarn
   type-check` flags: typed fixtures of `ApiKeyListEntry` that lack `allowed_cidrs`, and
   `makeConfig` in `ApiKeysAdminPage.test.tsx`, which needs `setAllowlist`.
9. Tests (see Testing Strategy), docs, and a CHANGELOG entry.

## Files to Modify

- `analytics-web-app/src/lib/api-keys-shared.ts`
- `analytics-web-app/src/lib/ingestion-api-keys-api.ts`
- `analytics-web-app/src/lib/analytics-api-keys-api.ts`
- `analytics-web-app/src/components/AllowedCidrsField.tsx` (new)
- `analytics-web-app/src/components/EditAllowlistDialog.tsx` (new)
- `analytics-web-app/src/components/ApiKeysAdminPage.tsx`
- `analytics-web-app/src/components/MintIngestionKeyDialog.tsx`
- `analytics-web-app/src/routes/AnalyticsApiKeysPage.tsx`
- `analytics-web-app/src/routes/IngestionApiKeysPage.tsx`
- Tests: `lib/__tests__/{ingestion,analytics}-api-keys-api.test.ts`, a new
  `lib/__tests__/api-keys-shared.test.ts`, `components/__tests__/ApiKeysAdminPage.test.tsx`,
  `routes/__tests__/IngestionApiKeysPage.test.tsx`, `routes/__tests__/AnalyticsApiKeysPage.test.tsx`,
  and `routes/__tests__/AudienceAccessPage.test.tsx`
- `mkdocs/docs/admin/api-keys.md`
- `CHANGELOG.md`

## Trade-offs

- **No client-side CIDR validation.** Validating in the browser would copy
  `IpAllowlist::parse`'s rules, including the host-bits rejection and IPv4-mapped IPv6 handling,
  into TypeScript, and the two copies could drift. The server already returns a precise message,
  and the dialogs already show server errors. The cost is a round trip before the user sees a
  typo.
- **Free-text textarea, not a chip or row list editor.** Allowlists are a handful of entries that
  operators often paste from elsewhere. A textarea with lenient splitting (newline, comma,
  whitespace) accepts every common pasted format, and it needs no new component library.
- **`allowed_cidrs` is required on `ApiKeyListEntry`, not optional as the issue sketched.** The
  server always sends it (`COALESCE(..., '{}')`), so the type is accurate, and the compiler finds
  every typed fixture that needs updating for it (untyped list-response mocks still need the
  manual pass in Implementation Steps step 8). An optional field would allow a meaningless third
  state, `undefined`.
- **Edit is a separate dialog, not inline editing in the cell.** This matches how the page
  already handles mint and revoke, and it gives the server's error message a place to appear.
- **No "only an admin can change this later" note in the self-service dialog.** A non-admin who
  gets the allowlist wrong can revoke the key and mint a new one. The note would be dialog text
  that only one of the three surfaces needs.

## Documentation

- `mkdocs/docs/admin/api-keys.md`:
  - `## IP allowlisting`: add that the allowlist can also be set in the web app. Mint dialogs on
    both admin pages and on Audience Access take it at mint time. Admins can edit it from the
    key tables with the shield action.
  - `## Web app admin pages`: mention the IP Allowlist column (empty means *Unrestricted*) and
    the edit action.
- `CHANGELOG.md` (Unreleased): one entry, "Show and edit API-key IP allowlists in the analytics
  web app (#1611)". No breaking-change clause: the `mint` options-object change is internal to
  the web app, not a published API.

## Testing Strategy

Everything here is reachable with unit tests (Vitest + Testing Library, with the network stubbed
through `global.fetch`, following the style of the existing tests). No live-DB test is needed.
The server side of the round trip is #1600's, and this change fixes no bug seen in the wild.

- `lib/__tests__/api-keys-shared.test.ts` (new), `parseAllowlistInput`:
  - Newline-, comma-, and mixed-whitespace-separated input.
  - Blank lines and trailing separators are dropped.
  - Empty or whitespace-only input returns `[]`.
  - Entries are passed through unchanged, with no normalization.
- `lib/__tests__/{ingestion,analytics}-api-keys-api.test.ts`:
  - `mint` with `allowed_cidrs` puts it in the POST body.
  - `mint` without it sends a body with no `allowed_cidrs` key.
  - `set*ApiKeyAllowlist` sends `PATCH /api/{table}-api-keys/<encoded id>/allowlist` with
    `{allowed_cidrs}` in the body, including the `[]` case.
- `components/__tests__/ApiKeysAdminPage.test.tsx`:
  - The column shows *Unrestricted* for `[]` and one line per entry otherwise.
  - The edit button is absent on revoked rows.
  - The edit dialog opens prefilled with the entries joined by newlines.
  - Save calls `setAllowlist(key_id, parsed)`, closes the dialog, and calls `listKeys` again.
  - Clearing the textarea saves `[]`.
  - A rejected `setAllowlist` keeps the dialog open and shows the error message.
  - The inline mint form passes `{ allowed_cidrs: [...] }` to `mintKey` when filled, and
    `allowed_cidrs: undefined` when blank.
- `routes/__tests__/AudienceAccessPage.test.tsx` (or `IngestionApiKeysPage.test.tsx`, which
  already captures the mint POST): filling the allowlist in `MintIngestionKeyDialog` puts
  `allowed_cidrs` in the POST body, and reopening the dialog clears the field.

## Manual Verification

The layout and wording are visual and nothing checks them automatically. A broken layout would
be obvious the first time anyone opens the page, so a manual check is enough.

1. Export `MICROMEGAS_OIDC_CONFIG` (or `MICROMEGAS_ANALYTICS_OIDC_CONFIG`) before running
   `start_services.py`; otherwise it starts with `--disable-auth` and every key-management
   route returns 503 AUTH_DISABLED. Then run
   `python3 local_test_env/ai_scripts/start_services.py --monolith`, open
   `http://127.0.0.1:3000/admin/ingestion-keys`, and sign in as an admin. Existing keys show
   *Unrestricted* in the IP Allowlist column.
2. Mint a key with `127.0.0.1` in the allowlist field. The new row lists `127.0.0.1`.
3. Use the shield action to change the entry to `10.0.0.5/8`. The dialog shows the "has host bits
   set" error and stays open. Change it to `10.0.0.0/8` and save. The row updates.
4. Clear the textarea and save. The row goes back to *Unrestricted*.
5. Repeat step 2 on `/admin/analytics-keys` and on `/audiences` (the self-service dialog) to
   confirm the field appears in both mint surfaces.

## Open Questions

None.
