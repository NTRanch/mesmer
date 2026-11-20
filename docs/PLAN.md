# MESMER — plan for the NTRanch erotic-hypnosis MCP server

> BLOCH: origin_lane=ntranch, content_label=nsfw.
> Commissioned by the Owner 2026-11-20 (planning mode + research).
> Name ruled by the Owner 2026-11-20: **mesmer** — for Franz Anton
> Mesmer; crates.io availability verified the same day (the name
> does not exist there). Registry home when built:
> io.github.NTRanch/mesmer. Lane law, per the hypno-technique
> skill: NTRanch/adult lane only — never in the ai.qompass
> registry, never routed to Azoth; public artifacts carry studio
> attribution (Nice Thoughts Ranch) only.

## 1. What mesmer is

An MCP server that **deterministically and generatively composes
hypnosis content packages** for the estate's apps and games:
complete sessions (induction → deepener → suggestion body →
anchor installation → emergence → aftercare), layered subliminal
editions of those sessions, visual-fixation specs, and
longitudinal conditioning programmes — every package emitted as
data (a `torogao-session/2` document plus audio-recipe and visual
specs) that a consumer renders. Mesmer composes and consents;
**torogao remains the singular build/runtime engine (D-457)** and
renders. Mesmer renders nothing itself.

The subject of every experience mesmer generates is **the end
user herself**: legally adult women who choose, inside a Nice
Thoughts Ranch app or game, to be hypnotized. The server's first
design problem is therefore not generation. It is consent —
§3 is the spine of this plan, and the generation canon (§4)
hangs off it.

**Scope rulings carried from the commission:**
- **Out of the canon, entirely:** sissification content and
  female-dominant content. These are not consent-gated classes;
  mesmer does not generate them at all.
- **In scope, as first-class consented kinks:** subliminal
  messaging, in-the-moment unawareness of conditioning, and
  longitudinal conditioning through Nice Thoughts Ranch — each
  handled by the envelope architecture in §3, which exists
  precisely because these kinks are preyed upon elsewhere: the
  safe way to experience them is the product.
- Estate copyright law (hypno-technique skill §1): studied
  corpus texts are never reproduced, shipped, or fed to
  generation. Everything mesmer ships is estate-original text
  composed from structural canon. Affirmation-set records carry
  provenance, and validation refuses a set whose provenance
  names a studied creator.

## 2. What already exists (the ground this stands on)

- **The technique canon is already studied and parameterised**:
  the hypno-technique skill (T1–T11, from the 1,777-file
  corpus programme, 2026-11-02) — SSB-AM encoding in fixed and
  adjustable-carrier forms, the layered-track architecture, the
  hidden-layer grammar, the drone layer, the T6 session block
  architecture, layer-subset editions, visual accompaniment,
  and the T11 null-calibrated detection discipline, all under
  Addition D production law (deterministic, seeded, 48 kHz,
  −16 LUFS/−1 dBTP, recipe hash + seed + licence on every
  render; a subliminal is specified by depth below the bed and
  refused, never clipped, past ceiling).
- **The torogao build landed** (e4cb21d1, 2026-11-07):
  `subliminal.rs` (SSB-AM encode + decode-verify),
  `HypnosisTrackBinding`, `torogao-session/2` typed session
  blocks, visual induction primitives. Mesmer's composer targets
  these formats; it does not re-implement them.
- **The market reference** (HypnoMaker, docs read 2026-11-20):
  a render-to-video tool with a 102-script library, TTS voices,
  a binaural layer, overlays (spirals, tunnels, vortex),
  three-panel and PMV modes — and, significantly, **grounding /
  decompression scripts that clear triggers and return the
  listener to baseline**. The market ships the off-ramp;
  mesmer canonises it as a first-class tool (§3.5).
- **The specimens** (primo `~/.GH/ph43d12u5/`, censused
  2026-11-20): `hott` — "Conditioning Control Panel" (.NET/WPF):
  flash images, mandatory videos, subliminals, session
  automation, gamification; `Induce-Hypnosis-with-Fractals-Audio`
  — a WebGL/WebAudio theta-entrainment PWA with synchronised
  fractal visuals; `dynamic_hypno_generator` — a Python dynamic
  TTS generator (layered tones, echoes, a looping mantra stem,
  live line display). Plus TranceParent (itch.io): a Unity
  self-hypnosis media-layer player. Study specimens for feature
  shape only, under D-1679: anything adopted is re-implemented
  in Rust.
- **Demand signals** (what is most sought, from the HypnoMaker
  shelf and the corpus's genre structure, minus the exclusions):
  affirmation and identity loops for background "everyday"
  listening; guided voice sessions with a real descent; trigger
  installation; layered subliminal editions of a session the
  listener already knows (T8); and programmes whose effect is
  framed as accumulating with repetition. §4 maps each to a
  mesmer capability.

## 3. The consent architecture (the spine)

Modelled on ache's proven shape — master off by default, ceiling
clamped, synchronous stop, no resume on reconnect, a pure
`decide()` gate — generalised from device safety to experiential
consent.

### 3.1 The consent profile is the consent act
Mesmer generates nothing for a user without a **consent
profile**: a typed, versioned record the user creates in an
alert state, beginning with an adult attestation and the plain
statement of what use means — *using this is consenting to be
hypnotized within the bounds recorded here*. The profile
enumerates: allowed effect classes (the §4 taxonomy), the user's
own additional exclusions (on top of §1's canon exclusions,
which no profile can remove), an intensity ceiling, a maximum
session length, and three **separate, default-off grants** —
subliminal layers, trigger persistence beyond a session, and
longitudinal programmes. Each grant is its own act; none rides
along with another.

### 3.2 The envelope: how consented unawareness works
The kink of being conditioned without knowing the specifics is
served by separating **what consent attaches to** from **what
the moment contains**. Consent attaches to the *envelope*: the
classes, themes, bounds, and ceiling in the profile. Payloads
inside the envelope need not be previewed — that unawareness is
the experience the user asked for. Three things are structural
and can never be waived, by any profile, programme, or session:

1. **Stop.** Every consumer surface must offer a synchronous
   stop that ends delivery immediately and fires the emergence
   block. A package mesmer emits carries this as a rendering
   obligation in its metadata; a consumer that cannot honour it
   does not get packages.
2. **The record.** Everything mesmer delivers is written to a
   **disclosure log** in full — every line, layer, and encoded
   payload, decodable by design (the T11 decode path exists in
   torogao as a gate instrument; here it is also the user's
   audit right). Unawareness is in-the-moment, never
   in-the-record. The user may choose not to read the log; she
   can never be without one.
3. **The exclusions.** `decide()` refuses any generation
   request outside the envelope — including requests arriving
   from an active longitudinal programme, and including classes
   smuggled under synonyms (the taxonomy is closed; unknown
   classes refuse, they do not pass).

### 3.3 Consent cannot grow in trance
Community practice is unambiguous: negotiation happens alert,
and consent is never expanded mid-session. Mesmer encodes it:
envelope-widening operations require a fresh alert-state act
and are refused while a session context is active. Narrowing —
lowering a ceiling, removing a class, revoking a grant — is
accepted at any time, from any state, effective immediately.

### 3.4 Longitudinal conditioning is an expiring grant
A programme (a scheduled sequence of sessions designed to
accumulate — spaced repetition against chosen anchors and
affirmation sets) exists only under the longitudinal grant, and:
it carries an **expiry** (default 28 days) after which it halts
until the user renews it in an alert state; it enforces the
user's own cadence caps (the community's working guidance is
roughly one session a day — over-listening flattens response,
and the programme treats the cap as the user's stated bound,
not a suggestion); its full history is exportable from the
disclosure log; and **one-call total revocation** ends the
programme, clears installed triggers (§3.5), and closes the
grant. Nothing about a programme is silent: enrolment,
renewal, and revocation are all user acts with records.

### 3.5 Grounding is a tool, not content
Every session package ends with emergence and an aftercare
block; every installed trigger is registered with its removal
procedure at installation time (triggers fade without
reinforcement, and mesmer ships the deliberate version of that
fade); and a standalone **grounding package** — trigger
clearing, baseline return, the market's "White Script" pattern
generalised into canon — is generatable on demand, at any time,
including as the automatic close of a revocation. No programme
or session can disable it.

### 3.6 Standing safety content rules
No suggestion class may target the user's ability to refuse,
stop, or remember the disclosure log's existence. Packages are
metadata-flagged never-for-driving (delivery while driving or
operating machinery is the genre's one documented physical
risk; consumers surface the flag). Health-adjacent claims are
not generated: mesmer composes erotic and experiential content,
never therapeutic promises.

## 4. The technique canon, graded honestly

The Owner's standard is evidence: the canon grades each
technique, and the composer leans on the load-bearing ones.

**Grade A — the mechanisms that carry the experience**
(focused attention, expectation, ritual structure, repetition,
personally resonant suggestion craft):
- *Session architecture* (estate T6): set-up and permission →
  induction → deepener → suggestion body → anchor installation
  → emergence → aftercare. The frame itself is the strongest
  effect in the genre.
- *Induction families*: eye fixation; progressive relaxation;
  fractionation (repeated emergence and re-descent, each cycle
  deeper — the genre's most reliable depth tool); confusion /
  overload; rapid induction (flagged: live-context technique,
  canonised for completeness, rarely the right package choice).
- *Suggestion craft*: pacing-and-leading; direct vs indirect
  (Ericksonian) forms; embedded commands; yes-sets; convincers
  (early small proofs that deepen belief in the process);
  future pacing; metaphor and story frames.
- *Anchors and triggers*: installation by pairing at peak,
  firing, scheduled reinforcement, registered removal (§3.5).
- *Affirmation loops* (estate T4 grammar): a small closed set,
  present-tense, first-person identity declarations and
  second-person imperatives, a fixed cue phrase heading each
  cycle, high repetition density — the background-listening
  form, and the longitudinal programme's workhorse.
- *Post-hypnotic framing with expiry*: suggestions scoped to a
  context and a lifetime, never open-ended by default.

**Grade B — estate-parameterised, real as construction**
(the layered-media machinery; its effects ride on Grade A):
- SSB-AM subliminal encoding (T1/T2) and buried-by-level layers;
  the drone layer (T5); layer-subset editions (T8) — one master
  session published as full / lite / subliminal-only mixes.
- Binaural pacing curves (Addition D: carrier + per-phase beat
  pacing) as an attention aid and session clock.
- Visual fixation accompaniment (T9): looping visual anchors,
  frame-accurate flash-text cues, seeded spiral/overlay specs —
  the fractal and spiral specimens (§2) inform parameters here.

**Grade C — genre theatre, labelled as such in the canon:**
- Binaural "entrainment" frequency tables (alpha/theta/delta
  as marketed) — the pacing curve is real tooling; the
  brainwave claims outrun the evidence and mesmer's canon says
  so wherever they appear.
- Subliminal *persuasion strength* as popularly claimed — the
  estate's own T11 discipline stands: hidden layers are genre
  practice whose power is expectation, ritual, and repetition;
  mesmer never promises a listener she will be changed against
  a preference she does not hold. This honesty is also the
  safety case: the envelope works because the user chose it.

**Effect-class taxonomy** (the consent profile's vocabulary;
closed, versioned, each class mapped to its canon techniques):
relaxation/descent · sensory amplification · sensory creation
(suggested touch, warmth, restriction) · arousal shaping ·
orgasm control (permission/denial framing) · identity
affirmation · submission headspace · fantasy immersion ·
trigger installation · amnesia framing (session-scoped only;
§3.2's record is disclosed as part of consenting to this class)
· accumulation/programme work. Sissification and femdom are
absent from the taxonomy by §1 ruling — not disabled classes,
nonexistent ones.

## 5. Canon as Rust: files, types, and tool calls

**Repository shape** (when built): NTRanch/mesmer — a Rust
workspace in the fleet pattern: `mesmer` lib crate + thin MCP
binary (rmcp, modern era only), its contained proc-macro derive
when the fleet rollout reaches it, agent card + server.json
frozen at one version per release.

**The canon is data, versioned in the repo** (`canon/`):
- `canon/effects.toml` — the §4 taxonomy: classes, grades,
  technique mappings, per-class bounds (max intensity, session
  scoping rules).
- `canon/blocks/` — session block templates per T6 (induction
  families, deepeners, emergence, aftercare, grounding), as
  structured data with typed slots — never free prose files.
- `canon/patterns.toml` — the suggestion-craft patterns
  (pacing/leading shapes, embedded-command frames, yes-set
  forms) as composable grammar rules.
- `canon/encoding.toml` — the T1/T2/T5/T7 parameters, imported
  from the technique spec (single source; the skill remains
  the human-readable record).
- Affirmation sets are **records, not files in the canon**:
  estate-original, versioned, carrying person/tense/theme
  metadata and the provenance field the validator enforces.

**Types enforce the contracts**: `ConsentProfile`,
`Envelope`, `SessionRequest`, `ProgrammeGrant`, and the
`decide()` gate are Rust types in the lib; a request that
cannot be typed inside an envelope cannot be composed. The
fleet's refusal taxonomy applies (typed refusals, e.g.
`MESMER-ENVELOPE-CLASS`, `MESMER-GRANT-ABSENT`,
`MESMER-PROGRAMME-EXPIRED`, `MESMER-WIDEN-IN-SESSION`), and
the 2026-11-20 reversibility addendum annotates every tool:
consent and programme operations are declared-reversible by
design — revocation is their named inverse, always available.

**Generation is deterministic**: composition is seeded
selection + grammar assembly over the canon; the same request
+ seed yields the byte-identical package, stamped with seed +
recipe hash (Addition D). Local models may draft *new canon
material* at authoring time (offline, gated by Pax/Owner,
frozen into canon data on admission) — runtime generation
never calls a model. This is what "deterministically and
generatively" means here: generative reach, deterministic
behaviour.

**Tool surface** (draft; bare profile sized to the fleet
budget, profiles for the rest):
- Consent: `consent_profile_create`, `consent_profile_get`,
  `consent_profile_amend` (narrowing free, widening gated per
  §3.3), `consent_revoke` (total, fires §3.5 close).
- Composition: `envelope_check` (a `decide()` preview — what
  would be refused and why), `session_compose` (request + seed
  → torogao-session/2 package + layer manifest + disclosure
  entry), `edition_derive` (T8 layer-subset editions of an
  existing package).
- Programmes: `programme_propose` (schedule + classes →
  grant-scoped plan), `programme_status`, `programme_revoke`.
- Grounding & record: `grounding_compose` (the §3.5 off-ramp,
  always available), `disclosure_export` (the user's log,
  with decode-verify applied to encoded layers on request).
- Canon: `canon_show` (taxonomy, grades, and bounds — the
  honesty of §4 is user-visible, not internal).

**Consumers**: Strobe first (Vivienne's audio vector is a
mesmer-shaped use case already in canon), then the hypnosis
titles in the rebuild programme (Hypnosis Card, hypno_booth)
and app surfaces. Consumers receive packages + rendering
obligations (stop surface, driving flag, log write-back).

## 6. Build rungs (when commissioned — planning mode ends here)

- **R1 — Canon data + types**: taxonomy, blocks, patterns,
  encoding parameters; consent types; the lib skeleton.
  Gate: canon validates against the technique spec's numbers;
  the taxonomy is closed (unknown class refuses at parse).
- **R2 — The gate**: `decide()` + refusal taxonomy + the
  consent tools. Gate: the adversarial half of the test suite
  — excluded class by synonym, subliminal without its grant,
  widening inside a session context, programme past expiry —
  every one refuses typed, none composes.
- **R3 — The composer**: seeded composition to
  torogao-session/2 + edition derivation. Gate: double-compose
  byte-identical; a composed session renders through torogao's
  existing hypno build without modification.
- **R4 — Programmes + disclosure**: scheduling, expiry,
  renewal, revocation-with-grounding, the exportable log.
  Gate: revocation from every programme state lands in the
  grounded, closed state; the log reconstructs every delivered
  package bit-for-bit.
- **R5 — First consumer**: Strobe integration behind its own
  gates; fleet publication motion (crates.io + Registry,
  io.github.NTRanch) under the standing per-push rules.

## 7. Open items for the Owner

None block R1. Two notes: (a) the GitHub repo creation under
NTRanch rides the build commission, as with cartone's
pre-authorised pattern — it is not assumed by this plan;
(b) the name's crates.io availability was verified
2026-11-20 — availability is a snapshot, and the build motion
re-verifies before publishing, per fleet practice.
