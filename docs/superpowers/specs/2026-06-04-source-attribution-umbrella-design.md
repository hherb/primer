# Per-source attribution umbrella (`parent_source_id`) — issue #40

**Date:** 2026-06-04
**Issue:** [#40 — data/ingest: aggregate per-source attribution for the Wikipedia layer](https://github.com/hherb/primer/issues/40)
**Status:** approved, ready for implementation

## Problem

Every Wikipedia/Klexikon passage emitted by `data/ingest` registers its own row
in the cross-locale `sources` table, with `id == source == "<id_prefix>:<pack>:<slug>"`
(e.g. `wiki-simple:en:mercury`). For N articles you get N source rows and there is
**no shared parent row** a credits UI could read to render a single
"Corpus from Simple English Wikipedia" line. Aggregated attribution currently
requires walking every `wiki-simple:en:*` row and de-duping by licence, which the
schema does not encode.

Issue #40 lists three options; the owner chose **option 1**: keep the per-passage
rows (so per-article retrieval filtering and per-article URLs are preserved) and
add an optional `parent_source_id` self-FK so all `wiki-simple:en:*` rows can point
at one shared `wiki-simple:en` umbrella row.

Two sub-decisions, both resolved with the owner:
- **Umbrella metadata is produced by the Python ingest (approach A)**, not derived
  in the Rust loader. The ingest is the only layer that knows the proper aggregated
  credit line; this matches the project's "data-as-data, user-facing strings out of
  Rust" discipline.
- **The umbrella is carried as a nested `parent_source` object on each passage
  (approach A)**, not via a separate header/sidecar record. Keeps the JSONL a
  single record type, order-independent, and self-contained per line. The hand-drafted
  seed corpus simply omits the field (it is `Option`), so flat seed passages opt out
  for free.

## Design

### 1. Schema + types (Rust)

- **`primer-knowledge` schema `USER_VERSION` 3 → 4.** `create_sources_table` gains a
  nullable self-referential column:
  ```sql
  parent_source_id TEXT REFERENCES sources(id)
  ```
  A fresh DB creates the table with the column. An existing v3 DB is migrated by a new
  idempotent `apply_v4`-style step (`ALTER TABLE sources ADD COLUMN parent_source_id TEXT`
  guarded by a `pragma_table_info` check — the codebase's column-add pattern), run inside
  the existing migration transaction in `migrate_or_create`. The self-FK is documentation
  only at the SQLite level (FKs are not enforced unless `PRAGMA foreign_keys=ON`); the
  loader is responsible for inserting parents.
- **`primer_core::knowledge::SourceMeta`** gains `pub parent_source_id: Option<String>`.
  `upsert_source` INSERT/UPDATE and `list_sources` SELECT extend to the 6th column.
  `parent_source_id` for an umbrella row is `None` (umbrellas have no parent).

### 2. Loader (`primer-kb-load`)

- **`SeedPassage`** gains `#[serde(default)] pub parent_source: Option<ParentSource>`,
  where `ParentSource { id, license, attribution, #[serde(default)] source_url }`
  is a new small struct.
- In `load_jsonl`'s existing source-dedup `HashMap<String, SourceMeta>` loop:
  - The **child** source's `SourceMeta.parent_source_id` is set to `Some(parent.id)`
    when the passage carries a `parent_source`, else `None`.
  - The **parent** is inserted into the same HashMap keyed by `parent.id`, with
    `parent_source_id: None`. De-dup is automatic — N children sharing one umbrella
    yield exactly one umbrella entry.
  - Both child and parent are written by the existing `for src in sources.values()`
    upsert loop. No new write path.
- Passages without `parent_source` behave exactly as today (`parent_source_id == None`).

### 3. Python emitter (`data/ingest`)

- `to_passage` adds a `parent_source` object built purely from the `WikiSource`:
  - `id`: `f"{source.id_prefix}:{source.pack_id}"`
  - `license`: `source.license`
  - `attribution`: `f"Corpus from {source.human_label}, licensed under {source.license} (per-article credits at the linked pages)"`
  - `source_url`: site root, derived robustly via a new pure helper
    `_site_root(url)` = `urllib.parse.urlsplit(web_base_url)` →
    `f"{scheme}://{netloc}/"` (NOT string-trimming `web_base_url`).
- No new `WikiSource` field. The hand-drafted seed JSONL is untouched.

### 4. Migration & idempotency

- Existing KB DBs: `apply_v4` adds the column (NULL for all existing rows), no data
  rewrite. Re-running the loader re-upserts umbrella + children idempotently via the
  existing `ON CONFLICT(id) DO UPDATE`.
- **No forced corpus regeneration.** The committed seed JSONL won't carry
  `parent_source` until a future `python3 simple_wikipedia.py` run regenerates it.
  Schema, loader, and Python all ship ready; nothing breaks before regeneration
  because the field is optional end-to-end.

## Testing (TDD, red→green)

**Rust — `primer-knowledge`:**
- Fresh DB lands at `USER_VERSION == 4` with a `parent_source_id` column on `sources`.
- A v3 DB (sources table without the column) migrates to v4 non-destructively (existing
  rows preserved, new column added, NULL).
- `upsert_source` + `list_sources` round-trip a `SourceMeta` carrying a
  `parent_source_id`.

**Rust — `primer-kb-load`:**
- JSONL with `parent_source` → a child source row with `parent_source_id = Some(umbrella)`
  AND a separate umbrella source row (`parent_source_id == None`).
- JSONL without `parent_source` → child row with `parent_source_id == None` (unchanged).
- Two passages sharing one umbrella → exactly one umbrella source row.

**Python — `data/ingest`:**
- `to_passage` emits the expected `parent_source` dict for `SIMPLE_ENGLISH` and `KLEXIKON`.
- `_site_root` unit tests: trailing `/wiki/` path stripped, port preserved, root already-root.
- Drift check: umbrella `id == f"{id_prefix}:{pack_id}"`.

## Out of scope (YAGNI)

- No `list_sources` aggregation/grouping API and no `--print-attribution` UI — #40 only
  needs the schema to *encode* the umbrella relationship.
- No backfill of the already-committed seed JSONL.
- No `WikiSource.home_url` field — the site root is derived.
