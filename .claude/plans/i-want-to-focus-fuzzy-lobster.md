# Stop tracking version in package.json / package-lock.json

## Context

CLAUDE.md's Versioning section currently instructs keeping `tauri.conf.json`, `package.json`, and
`package-lock.json` version fields in sync on every release bump. In practice this hasn't been
happening — the user has intentionally only updated `tauri.conf.json` (per prior guidance that it's
the single source of truth for a Tauri app), and an earlier session in this conversation
misdiagnosed that as a "drift bug." The user confirmed the intent and asked to verify it's safe to
drop the version field from the other files entirely, so this stops surfacing as a false positive.

**Verified this session, from `@tauri-apps/cli`'s own config schema** (`node_modules/@tauri-apps/cli/config.schema.json`):
`tauri.conf.json`'s `version` field is "a semver version number **or a path to a `package.json`
file**... If removed the version number from `Cargo.toml` is used." Since this repo's `version` is
a literal string, Tauri never reads `package.json` at all. Confirmed further:
- Nothing in `src/`/`src-tauri/` reads `package.json`'s version — the in-app version shown in
  `src/components/Sidebar.tsx:63-66` comes from `@tauri-apps/api/app`'s `getVersion()`, which
  resolves from `tauri.conf.json` via Tauri's Rust side.
- `.github/workflows/*.yml` (`test.yml`, `release.yml`) never reference `package.json`'s version.
- `package.json` already has `"private": true` — it was never going to be published to npm, where
  a `version` field actually matters.

So the field is purely cosmetic/inert today, and CLAUDE.md's "keep in sync" instruction is asking
for manual work that fixes nothing.

## Plan

1. **Remove the `"version"` field from `package.json`** (`/Users/nraboy/Desktop/restic-gui/package.json:4`).
2. **Regenerate `package-lock.json`** by running `npm install` (not hand-edited — npm owns this
   file's format and will drop/adjust the corresponding `version` fields, both the top-level one
   and `packages[""].version`, consistently on its own).
3. **Update CLAUDE.md's Versioning section** to reflect reality instead of the old 3-file-sync
   instruction:
   - State plainly that `src-tauri/tauri.conf.json`'s `version` is the **only** version that
     matters — Tauri reads it directly and never falls back to `package.json` (only to
     `Cargo.toml`, which CLAUDE.md already documents as deliberately pinned at `0.0.0`).
   - Note `package.json`/`package-lock.json` intentionally carry no `version` field, so there's
     nothing to keep in sync — a version-only release bump only touches `tauri.conf.json`.
   - Keep the existing `Cargo.toml` note (`0.0.0`, deliberately unused) as-is; it's still accurate
     and pairs naturally with this section now that the same "not the source of truth" framing
     applies to `package.json` too.

## Verification

- `npm install` completes cleanly and `package-lock.json` no longer carries mismatched version
  data (diff it to confirm the version fields were removed/adjusted, not left stale).
- `npm run typecheck`, `npm run lint`, `npm run build`, and `npm run test:vite` all still pass —
  confirms nothing in the toolchain silently depended on `package.json`'s version.
- Quick sanity check: `npm run tauri dev` (or at least `npm run tauri --help`) still resolves
  correctly with no version-related warnings.
- Re-read the updated CLAUDE.md Versioning section to confirm it no longer instructs a 3-file sync.
