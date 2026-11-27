# mesmer 1.1.0 — implementation report

Branch `pax/mesmer-1.1.0-20261127` off `df0c2c5` (1.0.0 tip).
Implements the playlists SPEC (`~/workspace/mesmer-playlists-20261127/SPEC.md`)
as ruled by the Owner 2026-11-27: playlists + composite delivery
(05:21 PT), concurrent sessions (06:40 PT), Q1 session lifecycle
adopted as spec'd (07:08 PT), Q2 budget — consolidations plus a
raise (07:09 PT). **Not published**: publishing 1.1.0 is a
separate later act.

## What landed, per spec section

- **§0 / concurrency revision** — the single session slot is now
  a handle-keyed map (`HashMap<Handle, Session>`) plus a runs
  map. Sessions on one profile, across profiles, and beside an
  active run are concurrent and independent (own cadence, own
  engagement). Lock order is sessions → runs everywhere.
  `decide()` / `decide_delivery()` are byte-unchanged.
  The per-profile scoping of the widening and playlist-authoring
  refusals is implemented as the spec's stated position and
  marked in code (`profile_amend`, `playlist_create`) as the
  one place a ruling change edits. Profile *creation* keeps the
  1.0.0 global alert-state rule (pinned by an existing test).
- **§1 playlists** — stored, typed, immutable artefacts bound to
  one profile (`<data>/playlists/<profile>/<id>.json`, validated
  against canon on every load; a file that no longer validates
  is `MESMER-STORE-CORRUPT`, never served). Each entry plays as
  a full session; closing an entry's session advances the run,
  judged fresh; a refused advance halts the run with the refusal
  recorded (`PlaylistRunHalted`). One active run per playlist
  (`MESMER-PLAYLIST-STATE`). Revocation kills the profile's
  runs, sessions, and playlists in the same act.
- **§2 compounding** — typed per entry: `builds_on` strictly
  backward, no duplicates; declared `depth_target`s
  non-decreasing with steps ≤ canon `max_depth_step`; a
  compounding claim requires the compounded entries' deliveries
  to persist, and a claim declared over any session-only class
  is `MESMER-PLAYLIST-SCOPE`; build anchors must be installed
  by an earlier entry of the same playlist whose delivery
  persists (anchor continuity is run-local — concurrent
  sessions never feed it, test a13). The end state must be
  reachable as authored: every end class delivered by some
  entry, every end anchor installed, and the final entry's
  declared depth equal to the end depth. Completion is recorded
  as delivered-as-authored (SPEC §2.4 honesty clause).
- **§3 composite delivery** — ≤4 layers in the closed role set
  (exactly one bed, ≤1 subliminal, ≤1 visual, ≤1 drone); an
  anchor may ride only the bed layer; judged once: per-layer
  `decide()` plus one `decide_delivery()` on the union request
  (`gate::decide_composite`). One verdict, one disclosure
  record carrying every layer's full payload, one unit of
  session cadence (Invariant 0 holds: `cadence_cap_per_session`
  stays 1).
- **§4.1 canon** — new `canon/playlists.toml` (schema 1;
  bounds 12 entries / 4 layers / depth step 3; the closed layer
  role set), loaded and validated like every canon file.
  `effects.toml` untouched.
- **§4.3 gate** — new pure judgements beside the untouched
  pair: `decide_playlist_create`, `judge_playlist` (per-entry
  `decide()` + end-state terms), `decide_playlist_start`
  (start-level check + programme day cadence), `decide_composite`.
  New refusal codes: `MESMER-PLAYLIST-INVALID`,
  `MESMER-PLAYLIST-SCOPE`, `MESMER-PLAYLIST-STATE`,
  `MESMER-PLAYLIST-UNKNOWN`, `MESMER-ENDSTATE`.
  The programme day cap binds at start and at every entry
  opening, counted from the disclosure log (UTC day).
- **§4.4 tools** — six new tools (15 total): `session_open`,
  `session_close`, `playlist_create`, `playlist_show`,
  `playlist_start`, `composite_deliver`. Q1 consolidation as
  ruled: no separate `playlist_stop` — a run's stop is
  `session_close` with `halt: true` (a live run always holds
  its entry session, so nothing is lost by the fold).
  `playlist_show` stays its own tool (its fold into
  `playlist_start` would have lost run-state reads).
- **§4.5 store / disclosure** — new events: `SessionOpened`,
  `SessionClosed`, `PlaylistCreated`, `PlaylistDeleted`,
  `PlaylistRunStarted`, `PlaylistEntryOpened`,
  `PlaylistRunHalted`, `PlaylistRunCompleted`. Openings and
  entry openings are disclosed before handles are returned
  (fail-closed, as every mesmer act). The session opened by
  profile creation records no `SessionOpened` — `ProfileCreated`
  is its record, and 1.0.0 disclosure shapes are unchanged.

## Gates (primo, 2026-11-27)

| Gate | Result |
|---|---|
| `cargo test` | **148 passed, 0 failed** (122 at 1.0.0 + 26 new in `tests/r6_playlists.rs`, 13 validation / 13 adversarial per SPEC §5) |
| `cargo clippy --all-targets -- -D warnings` | clean (rc 0) |
| `cargo fmt --check` | clean |
| Theodolite (cured protocol, copy route) | **PASS** — clean leg 0 diagnostics / 9 files; planted E0425 control caught (`src/gate.rs:143:25`, 1 error); restored leg 0, byte-identical (sha256 `c337cd77…`) |
| tools/list budget (Q2) | measured **3,319** cl100k canonical (15 tools, 13,984 bytes, sha256 `f884d9df…`); new cap **3,400** per the 07:09 PT ruling (old cap 2,500; 1.0.0 measured 1,745) |

Two pre-existing tests were updated mechanically for the
surface growth, assertions otherwise untouched: the wire test's
tool count (9 → 15) and the canon fixture's `CanonSources`
literal (the new `playlists` field the spec mandates).

## Notes for the record

- The session-only compounding witness (test a7) is a
  composite: canon itself forbids a pack from persisting a
  session-only class (r1 pins that), so the dishonest claim
  arises only when a persisting bed is unioned with a
  session-only layer — refused `MESMER-PLAYLIST-SCOPE` at
  authoring and at composite delivery.
- Version 1.1.0 holds only because 1.0.0 never published.
  Cargo, `server.json`, and the agent card all carry 1.1.0;
  the agent card lists the 15 tools.
