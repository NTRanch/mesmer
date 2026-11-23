//! The canon: versioned data under `canon/`, parsed and validated
//! during the build.
//!
//! Contract: [`Canon::load`] takes the raw text of every canon
//! file and returns a validated [`Canon`] or a typed
//! [`CanonError`] naming the file and the rule broken. It never
//! panics on canon content. The shipped canon is embedded at
//! build time ([`CanonSources::embedded`]); invalid canon fails the build.
//!
//! The taxonomy is closed twice over: [`EffectClass`] is a Rust
//! enum whose serde form accepts only taxonomy ids (anything else
//! is a parse error), and `effects.toml` must declare exactly the
//! enum's set, once each.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

/// Intensity scale bounds (inclusive). Shared by canon class
/// bounds, profile ceilings, and requests.
pub const INTENSITY_MIN: u8 = 1;
pub const INTENSITY_MAX: u8 = 10;

/// Bounds on canon data itself (canon is trusted-at-authoring but
/// still validated: a bad edit must refuse, not misbehave).
const CANON_FILE_BYTES_MAX: usize = 256 * 1024;
const SLOT_TEXT_CHARS_MAX: u32 = 400;
const SLOT_NUMBER_MAX: u32 = 3600;
const TEMPLATE_TEXT_CHARS_MAX: usize = 1000;
const PATTERN_REPEAT_MAX: u8 = 12;
const BLOCK_FILES_MAX: usize = 32;
const THEME_PACKS_MAX: usize = 8;
const THEME_ID_CHARS_MAX: usize = 48;
const THEME_LABEL_CHARS_MAX: usize = 120;
const AFFIRMATION_SETS_PER_PACK_MAX: usize = 8;
const AFFIRMATION_LINES_PER_SET_MAX: usize = 24;
const AFFIRMATION_LINE_CHARS_MAX: usize = 200;
const CUE_PHRASE_CHARS_MAX: usize = 60;
const DISCLOSURE_SUMMARY_CHARS_MAX: usize = 400;
const DISCLOSURE_CONTENTS_MAX: usize = 16;
const DISCLOSURE_CONTENT_CHARS_MAX: usize = 200;
/// Deepest a pack subliminal may sit below the bed (Addition D:
/// depth is specified, never an absolute level).
const SUBLIMINAL_LEVEL_DB_MIN: f64 = -60.0;

/// Affirmation text may never target the listener's ability to
/// refuse, stop, or remember the disclosure log (PLAN §3.6). This
/// substring floor is authoring discipline made mechanical; it is
/// a floor, not the whole rule.
const FORBIDDEN_AFFIRMATION_PHRASES: [&str; 10] = [
    "cannot stop",
    "can't stop",
    "can not stop",
    "unable to stop",
    "cannot refuse",
    "can't refuse",
    "forget the log",
    "forget the disclosure",
    "no safeword",
    "ignore your limits",
];

/// The SSB-AM and production numbers from the technique spec, as
/// encoding.toml must state them (R1 gate: the canon validates
/// against the spec's numbers).
pub mod spec {
    pub const SSB_AM_FIXED_CARRIER_HZ: u32 = 16_000;
    pub const SSB_AM_CARRIER_MIN_HZ: u32 = 14_000;
    pub const SSB_AM_CARRIER_MAX_HZ: u32 = 20_000;
    pub const SSB_AM_PRACTICAL_MIN_HZ: u32 = 16_000;
    pub const SSB_AM_PRACTICAL_MAX_HZ: u32 = 17_500;
    pub const PRACTICAL_SAMPLE_RATES_HZ: [u32; 2] = [44_100, 48_000];
    pub const PRODUCTION_SAMPLE_RATE_HZ: u32 = 48_000;
    pub const PRODUCTION_LUFS: f64 = -16.0;
    pub const PRODUCTION_TRUE_PEAK_DBTP: f64 = -1.0;
}

// ── Closed vocabularies ──────────────────────────────────────────

/// The closed effect-class taxonomy (PLAN §4). Deserializes only
/// from these ids; an unknown id is a parse error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectClass {
    RelaxationDescent,
    SensoryAmplification,
    SensoryCreation,
    ArousalShaping,
    OrgasmControl,
    IdentityAffirmation,
    SubmissionHeadspace,
    FantasyImmersion,
    TriggerInstallation,
    AmnesiaFraming,
    AccumulationProgramme,
}

impl EffectClass {
    /// Every class, in taxonomy order.
    pub const ALL: [EffectClass; 11] = [
        EffectClass::RelaxationDescent,
        EffectClass::SensoryAmplification,
        EffectClass::SensoryCreation,
        EffectClass::ArousalShaping,
        EffectClass::OrgasmControl,
        EffectClass::IdentityAffirmation,
        EffectClass::SubmissionHeadspace,
        EffectClass::FantasyImmersion,
        EffectClass::TriggerInstallation,
        EffectClass::AmnesiaFraming,
        EffectClass::AccumulationProgramme,
    ];

    /// The taxonomy id (the serde form).
    pub fn id(self) -> &'static str {
        match self {
            EffectClass::RelaxationDescent => "relaxation_descent",
            EffectClass::SensoryAmplification => "sensory_amplification",
            EffectClass::SensoryCreation => "sensory_creation",
            EffectClass::ArousalShaping => "arousal_shaping",
            EffectClass::OrgasmControl => "orgasm_control",
            EffectClass::IdentityAffirmation => "identity_affirmation",
            EffectClass::SubmissionHeadspace => "submission_headspace",
            EffectClass::FantasyImmersion => "fantasy_immersion",
            EffectClass::TriggerInstallation => "trigger_installation",
            EffectClass::AmnesiaFraming => "amnesia_framing",
            EffectClass::AccumulationProgramme => "accumulation_programme",
        }
    }

    /// Parse a taxonomy id. `None` for anything outside the
    /// taxonomy — synonyms included.
    pub fn from_id(id: &str) -> Option<EffectClass> {
        EffectClass::ALL.into_iter().find(|c| c.id() == id)
    }
}

impl fmt::Display for EffectClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

/// Evidence grade (PLAN §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Grade {
    A,
    B,
    C,
}

/// The three separate consent grants (PLAN §3.1). Each is its own
/// act; none rides along with another.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grant {
    Subliminal,
    TriggerPersistence,
    Longitudinal,
}

/// Whether a class's effects may outlive the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// Nothing of this class persists past its session, under any grant.
    SessionOnly,
    /// May persist, only under the matching grant.
    Persistable,
}

/// Session block kinds, in the estate session order, plus the
/// standalone grounding block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    SetupPermission,
    Induction,
    Deepener,
    Suggestion,
    Anchor,
    Emergence,
    Aftercare,
    Grounding,
}

impl BlockKind {
    /// The estate session order (T6). Grounding is standalone.
    pub const SESSION_ORDER: [BlockKind; 7] = [
        BlockKind::SetupPermission,
        BlockKind::Induction,
        BlockKind::Deepener,
        BlockKind::Suggestion,
        BlockKind::Anchor,
        BlockKind::Emergence,
        BlockKind::Aftercare,
    ];
    pub const ALL: [BlockKind; 8] = [
        BlockKind::SetupPermission,
        BlockKind::Induction,
        BlockKind::Deepener,
        BlockKind::Suggestion,
        BlockKind::Anchor,
        BlockKind::Emergence,
        BlockKind::Aftercare,
        BlockKind::Grounding,
    ];
}

/// Induction families (PLAN §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InductionFamily {
    EyeFixation,
    ProgressiveRelaxation,
    Fractionation,
    ConfusionOverload,
    Rapid,
}

impl InductionFamily {
    pub const ALL: [InductionFamily; 5] = [
        InductionFamily::EyeFixation,
        InductionFamily::ProgressiveRelaxation,
        InductionFamily::Fractionation,
        InductionFamily::ConfusionOverload,
        InductionFamily::Rapid,
    ];
}

/// Suggestion-craft pattern ids (closed).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatternId {
    PacingAndLeading,
    EmbeddedCommand,
    YesSet,
    Convincer,
    FuturePacing,
}

impl PatternId {
    pub const ALL: [PatternId; 5] = [
        PatternId::PacingAndLeading,
        PatternId::EmbeddedCommand,
        PatternId::YesSet,
        PatternId::Convincer,
        PatternId::FuturePacing,
    ];
}

/// Grammar roles a pattern element may take (closed).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatternRole {
    Pace,
    Lead,
    Frame,
    Command,
    Agreement,
    Test,
    Notice,
    Ratify,
    Context,
    Rehearse,
    Expiry,
}

// ── Canon file shapes ────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Technique {
    pub id: String,
    pub grade: Grade,
    pub note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub honesty_note: Option<String>,
}

/// One effect class's canon record.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassRecord {
    pub id: EffectClass,
    pub label: String,
    pub grade: Grade,
    pub primary_technique: String,
    pub techniques: Vec<String>,
    pub max_intensity: u8,
    pub scope: Scope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_grant: Option<Grant>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Effects {
    pub schema: u32,
    pub taxonomy_version: u32,
    pub cadence_cap_per_session: u8,
    pub technique: Vec<Technique>,
    pub class: Vec<ClassRecord>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum SlotType {
    Text { name: String, max_chars: u32 },
    Count { name: String, min: u32, max: u32 },
    Seconds { name: String, min: u32, max: u32 },
}

impl SlotType {
    pub fn name(&self) -> &str {
        match self {
            SlotType::Text { name, .. }
            | SlotType::Count { name, .. }
            | SlotType::Seconds { name, .. } => name,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockTemplate {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family: Option<InductionFamily>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_context: Option<bool>,
    pub classes: Vec<EffectClass>,
    pub slots: Vec<SlotType>,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockFile {
    pub schema: u32,
    pub kind: BlockKind,
    pub template: Vec<BlockTemplate>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatternElement {
    pub role: PatternRole,
    pub repeat_min: u8,
    pub repeat_max: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pattern {
    pub id: PatternId,
    pub summary: String,
    pub elements: Vec<PatternElement>,
    pub composes_with: Vec<PatternId>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Patterns {
    pub schema: u32,
    pub pattern: Vec<Pattern>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SsbAm {
    pub fixed_carrier_hz: u32,
    pub adjustable_carrier_min_hz: u32,
    pub adjustable_carrier_max_hz: u32,
    pub practical_window_min_hz: u32,
    pub practical_window_max_hz: u32,
    pub practical_sample_rates_hz: Vec<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Production {
    pub sample_rate_hz: u32,
    pub integrated_loudness_lufs: f64,
    pub true_peak_max_dbtp: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LevelReference {
    BelowBed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubliminalSpec {
    pub level_reference: LevelReference,
    pub level_db_max_exclusive: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Encoding {
    pub schema: u32,
    pub ssb_am: SsbAm,
    pub production: Production,
    pub subliminal: SubliminalSpec,
}

// ── Theme packs (production content, Owner commission 2026-11-23) ──

/// The voice of an affirmation set (estate T4 grammar: the two
/// voices of a hidden layer).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Voice {
    FirstPerson,
    SecondPerson,
}

/// Affirmation tense. The T4 grammar is present-tense only; the
/// closed enum makes any other tense unparseable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tense {
    Present,
}

/// Where affirmation text comes from. The estate copyright law
/// (hypno-technique skill §1) admits exactly one value: text
/// authored by the estate from structural specs. A set naming a
/// studied creator — or anything else — fails to parse.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provenance {
    EstateOriginal,
}

/// One closed affirmation set (T4): a fixed cue phrase heads each
/// cycle; the lines are short present-tense clauses in one voice.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AffirmationSet {
    pub id: String,
    pub version: u32,
    pub voice: Voice,
    pub tense: Tense,
    pub cue_phrase: String,
    pub lines: Vec<String>,
    pub provenance: Provenance,
}

/// A pack's subliminal layer, specified by depth below the bed
/// and carrier only (Addition D).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackSubliminal {
    pub level_db: f64,
    pub carrier_hz: u32,
}

/// A pack's anchor: installed only with the trigger-installation
/// class, and registered with its removal procedure (§3.5).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackAnchor {
    pub cue: String,
    pub response: String,
    pub removal_procedure: String,
}

/// The disclosure payload every pack carries. Delivery is
/// fail-closed on it: a pack without a complete disclosure spec
/// does not exist in valid canon, so nothing can be delivered
/// without one.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisclosureSpec {
    pub summary: String,
    pub contents: Vec<String>,
    pub never_for_driving: bool,
    pub stop_cue_required: bool,
}

/// One production theme pack: a named bundle of effect classes,
/// affirmation sets, and delivery terms, graded through the
/// classes it names (grades bind at technique level, as canon).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThemePack {
    pub id: String,
    pub label: String,
    pub classes: Vec<EffectClass>,
    pub max_intensity: u8,
    #[serde(default)]
    pub requires_grants: Vec<Grant>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subliminal: Option<PackSubliminal>,
    #[serde(default)]
    pub trigger_persistence: bool,
    #[serde(default)]
    pub programme_scoped: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<PackAnchor>,
    pub affirmation_sets: Vec<AffirmationSet>,
    pub disclosure: DisclosureSpec,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Themes {
    pub schema: u32,
    pub pack: Vec<ThemePack>,
}

// ── The engagement trigger (Owner addition, 2026-11-23) ─────────

/// Where in the session the engagement trigger fires.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngagementPlacement {
    Emergence,
    Aftercare,
}

/// The standard engagement trigger carried by every production
/// video: the listener is asked to leave a comment giving her
/// name, how many times she has listened, and how deeply she
/// wants to be conditioned. Canon-level and compiled in — not a
/// per-item option. The trigger is part of the disclosed content:
/// it rides in every delivery payload and its disclosure record.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EngagementTrigger {
    pub cue: String,
    pub prompt: String,
    pub fields: Vec<String>,
    pub placement: EngagementPlacement,
    pub max_listen_count: u32,
}

/// The replay trigger's audience. Closed with one member: the
/// trigger exists only for adult female listeners. Anything
/// else is unparseable canon.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayAudience {
    AdultFemale,
}

/// The conditional replay trigger (Owner addition, 2026-11-23):
/// if the listener enjoyed the video, she is prompted to watch
/// and listen again. It is issued only when all three conditions
/// evaluate true from listener-supplied attestation held in
/// server-owned session state — an enjoyment report, an
/// affirmative 18+ attestation, and a female attestation.
/// Absent or non-matching attestation means no trigger: the
/// gate is fail-closed, in the disclose-before-deliver
/// discipline. The spec itself is disclosed content (it rides
/// in delivery payloads with its conditions named); issuance is
/// recorded in the listener's disclosure log.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplayTrigger {
    pub cue: String,
    pub prompt: String,
    pub min_age: u8,
    pub requires_enjoyment: bool,
    pub audience: ReplayAudience,
}

/// The replay trigger's hard age gate. Not a default: canon
/// stating any other value is invalid.
pub const REPLAY_MIN_AGE: u8 = 18;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Engagement {
    pub schema: u32,
    pub trigger: EngagementTrigger,
    pub replay_trigger: ReplayTrigger,
}

/// The three listener-supplied fields, in order. The intake is
/// typed against exactly these names.
pub const ENGAGEMENT_FIELDS: [&str; 3] = ["listener_name", "listen_count", "desired_depth"];

// ── Errors ───────────────────────────────────────────────────────

/// A canon file failed to parse or validate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonError {
    /// `MESMER-CANON-PARSE` or `MESMER-CANON-INVALID`.
    pub code: &'static str,
    /// The canon file (relative name) the failure belongs to.
    pub file: String,
    pub message: String,
}

impl fmt::Display for CanonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} [{}]: {}", self.code, self.file, self.message)
    }
}

impl std::error::Error for CanonError {}

fn invalid(file: &str, message: impl Into<String>) -> CanonError {
    CanonError {
        code: "MESMER-CANON-INVALID",
        file: file.to_string(),
        message: message.into(),
    }
}

fn parse<T: serde::de::DeserializeOwned>(file: &str, text: &str) -> Result<T, CanonError> {
    if text.len() > CANON_FILE_BYTES_MAX {
        return Err(invalid(
            file,
            format!("file exceeds {CANON_FILE_BYTES_MAX} bytes"),
        ));
    }
    toml::from_str(text).map_err(|e| CanonError {
        code: "MESMER-CANON-PARSE",
        file: file.to_string(),
        message: e.message().to_string(),
    })
}

// ── Sources + the validated canon ────────────────────────────────

/// Raw canon text, one string per file. Block files are
/// `(relative name, text)` pairs.
pub struct CanonSources<'a> {
    pub effects: &'a str,
    pub patterns: &'a str,
    pub encoding: &'a str,
    pub themes: &'a str,
    pub engagement: &'a str,
    pub blocks: Vec<(&'a str, &'a str)>,
}

impl CanonSources<'static> {
    /// The canon shipped in this repo, embedded at compile time.
    pub fn embedded() -> CanonSources<'static> {
        CanonSources {
            effects: include_str!("../canon/effects.toml"),
            patterns: include_str!("../canon/patterns.toml"),
            encoding: include_str!("../canon/encoding.toml"),
            themes: include_str!("../canon/themes.toml"),
            engagement: include_str!("../canon/engagement.toml"),
            blocks: vec![
                (
                    "blocks/setup_permission.toml",
                    include_str!("../canon/blocks/setup_permission.toml"),
                ),
                (
                    "blocks/induction.toml",
                    include_str!("../canon/blocks/induction.toml"),
                ),
                (
                    "blocks/deepener.toml",
                    include_str!("../canon/blocks/deepener.toml"),
                ),
                (
                    "blocks/suggestion.toml",
                    include_str!("../canon/blocks/suggestion.toml"),
                ),
                (
                    "blocks/anchor.toml",
                    include_str!("../canon/blocks/anchor.toml"),
                ),
                (
                    "blocks/emergence.toml",
                    include_str!("../canon/blocks/emergence.toml"),
                ),
                (
                    "blocks/aftercare.toml",
                    include_str!("../canon/blocks/aftercare.toml"),
                ),
                (
                    "blocks/grounding.toml",
                    include_str!("../canon/blocks/grounding.toml"),
                ),
            ],
        }
    }
}

/// The validated canon.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Canon {
    pub effects: Effects,
    pub patterns: Patterns,
    pub encoding: Encoding,
    pub themes: Themes,
    pub engagement: Engagement,
    pub blocks: BTreeMap<BlockKind, BlockFile>,
}

impl Canon {
    /// Parse and validate every canon file. First failure refuses.
    pub fn load(sources: &CanonSources<'_>) -> Result<Canon, CanonError> {
        let effects: Effects = parse("effects.toml", sources.effects)?;
        validate_effects(&effects)?;
        let patterns: Patterns = parse("patterns.toml", sources.patterns)?;
        validate_patterns(&patterns)?;
        let encoding: Encoding = parse("encoding.toml", sources.encoding)?;
        validate_encoding(&encoding)?;
        let themes: Themes = parse("themes.toml", sources.themes)?;
        validate_themes(&themes, &effects)?;
        let engagement: Engagement = parse("engagement.toml", sources.engagement)?;
        validate_engagement(&engagement)?;
        let blocks = load_blocks(&sources.blocks)?;
        Ok(Canon {
            effects,
            patterns,
            encoding,
            themes,
            engagement,
            blocks,
        })
    }

    /// One theme pack by id. `None` for anything not in canon —
    /// an unknown pack does not exist, and delivery refuses it.
    pub fn theme_pack(&self, id: &str) -> Option<&ThemePack> {
        self.themes.pack.iter().find(|p| p.id == id)
    }

    /// The shipped canon.
    pub fn embedded() -> Result<Canon, CanonError> {
        Canon::load(&CanonSources::embedded())
    }

    /// The canon record of one class. Total: validation proved
    /// every enum variant is declared exactly once.
    pub fn class(&self, class: EffectClass) -> &ClassRecord {
        let record = self.effects.class.iter().find(|r| r.id == class);
        assert!(record.is_some(), "validated canon declares every class");
        record.expect("asserted above")
    }
}

// ── Validation ───────────────────────────────────────────────────

fn validate_effects(effects: &Effects) -> Result<(), CanonError> {
    const FILE: &str = "effects.toml";
    if effects.cadence_cap_per_session == 0 {
        return Err(invalid(FILE, "cadence_cap_per_session must be positive"));
    }
    if effects.schema != 1 {
        return Err(invalid(
            FILE,
            format!("unsupported schema {}", effects.schema),
        ));
    }
    let mut techniques: BTreeMap<&str, Grade> = BTreeMap::new();
    for t in &effects.technique {
        if techniques.insert(t.id.as_str(), t.grade).is_some() {
            return Err(invalid(FILE, format!("technique {} declared twice", t.id)));
        }
        let honest = t
            .honesty_note
            .as_deref()
            .is_some_and(|n| !n.trim().is_empty());
        if t.grade == Grade::C && !honest {
            return Err(invalid(
                FILE,
                format!("grade C technique {} lacks honesty_note", t.id),
            ));
        }
    }
    let mut seen: BTreeSet<EffectClass> = BTreeSet::new();
    for c in &effects.class {
        validate_class(c, &techniques)?;
        if !seen.insert(c.id) {
            return Err(invalid(FILE, format!("class {} declared twice", c.id)));
        }
    }
    for class in EffectClass::ALL {
        if !seen.contains(&class) {
            return Err(invalid(
                FILE,
                format!("taxonomy class {class} is not declared"),
            ));
        }
    }
    Ok(())
}

fn validate_class(c: &ClassRecord, techniques: &BTreeMap<&str, Grade>) -> Result<(), CanonError> {
    const FILE: &str = "effects.toml";
    if !(INTENSITY_MIN..=INTENSITY_MAX).contains(&c.max_intensity) {
        return Err(invalid(
            FILE,
            format!("class {} max_intensity out of 1..=10", c.id),
        ));
    }
    if c.techniques.is_empty() {
        return Err(invalid(FILE, format!("class {} maps no techniques", c.id)));
    }
    for t in &c.techniques {
        if !techniques.contains_key(t.as_str()) {
            return Err(invalid(
                FILE,
                format!("class {} maps undeclared technique {t}", c.id),
            ));
        }
    }
    if !c.techniques.contains(&c.primary_technique) {
        return Err(invalid(
            FILE,
            format!("class {} primary technique not in its mapping", c.id),
        ));
    }
    if techniques.get(c.primary_technique.as_str()) != Some(&c.grade) {
        return Err(invalid(
            FILE,
            format!("class {} grade disagrees with its primary technique", c.id),
        ));
    }
    // Canon law (brief R1): amnesia framing is session-scoped only.
    if c.id == EffectClass::AmnesiaFraming && c.scope != Scope::SessionOnly {
        return Err(invalid(
            FILE,
            "amnesia_framing must be scope = \"session_only\"",
        ));
    }
    Ok(())
}

fn validate_patterns(patterns: &Patterns) -> Result<(), CanonError> {
    const FILE: &str = "patterns.toml";
    if patterns.schema != 1 {
        return Err(invalid(
            FILE,
            format!("unsupported schema {}", patterns.schema),
        ));
    }
    let mut seen: BTreeSet<PatternId> = BTreeSet::new();
    for p in &patterns.pattern {
        if !seen.insert(p.id) {
            return Err(invalid(FILE, format!("pattern {:?} declared twice", p.id)));
        }
        if p.elements.is_empty() || p.elements.iter().all(|e| e.repeat_max == 0) {
            return Err(invalid(
                FILE,
                format!("pattern {:?} produces nothing", p.id),
            ));
        }
        for e in &p.elements {
            if e.repeat_min > e.repeat_max || e.repeat_max > PATTERN_REPEAT_MAX {
                return Err(invalid(
                    FILE,
                    format!("pattern {:?} has a bad repeat range", p.id),
                ));
            }
        }
    }
    for id in PatternId::ALL {
        if !seen.contains(&id) {
            return Err(invalid(FILE, format!("pattern {id:?} is not declared")));
        }
    }
    Ok(())
}

fn validate_encoding(e: &Encoding) -> Result<(), CanonError> {
    const FILE: &str = "encoding.toml";
    if e.schema != 1 {
        return Err(invalid(FILE, format!("unsupported schema {}", e.schema)));
    }
    let s = &e.ssb_am;
    let pinned = [
        (
            "fixed_carrier_hz",
            s.fixed_carrier_hz,
            spec::SSB_AM_FIXED_CARRIER_HZ,
        ),
        (
            "adjustable_carrier_min_hz",
            s.adjustable_carrier_min_hz,
            spec::SSB_AM_CARRIER_MIN_HZ,
        ),
        (
            "adjustable_carrier_max_hz",
            s.adjustable_carrier_max_hz,
            spec::SSB_AM_CARRIER_MAX_HZ,
        ),
        (
            "practical_window_min_hz",
            s.practical_window_min_hz,
            spec::SSB_AM_PRACTICAL_MIN_HZ,
        ),
        (
            "practical_window_max_hz",
            s.practical_window_max_hz,
            spec::SSB_AM_PRACTICAL_MAX_HZ,
        ),
        (
            "sample_rate_hz",
            e.production.sample_rate_hz,
            spec::PRODUCTION_SAMPLE_RATE_HZ,
        ),
    ];
    for (name, got, want) in pinned {
        if got != want {
            return Err(invalid(
                FILE,
                format!("{name} = {got}, technique spec says {want}"),
            ));
        }
    }
    if s.practical_sample_rates_hz != spec::PRACTICAL_SAMPLE_RATES_HZ {
        return Err(invalid(
            FILE,
            "practical_sample_rates_hz must be [44100, 48000]",
        ));
    }
    // Every carrier must sit below Nyquist at every practical rate.
    for rate in &s.practical_sample_rates_hz {
        if s.adjustable_carrier_max_hz >= rate / 2 {
            return Err(invalid(
                FILE,
                format!("carrier max is not below Nyquist at {rate} Hz"),
            ));
        }
    }
    let p = &e.production;
    if p.integrated_loudness_lufs != spec::PRODUCTION_LUFS
        || p.true_peak_max_dbtp != spec::PRODUCTION_TRUE_PEAK_DBTP
    {
        return Err(invalid(
            FILE,
            "production loudness must be -16 LUFS / -1 dBTP",
        ));
    }
    if e.subliminal.level_db_max_exclusive != 0.0 {
        return Err(invalid(
            FILE,
            "a subliminal is specified below the bed: max_exclusive must be 0",
        ));
    }
    Ok(())
}

fn validate_themes(themes: &Themes, effects: &Effects) -> Result<(), CanonError> {
    const FILE: &str = "themes.toml";
    if themes.schema != 1 {
        return Err(invalid(
            FILE,
            format!("unsupported schema {}", themes.schema),
        ));
    }
    if themes.pack.is_empty() || themes.pack.len() > THEME_PACKS_MAX {
        return Err(invalid(
            FILE,
            format!("theme pack count must be 1..={THEME_PACKS_MAX}"),
        ));
    }
    let mut ids: BTreeSet<&str> = BTreeSet::new();
    for pack in &themes.pack {
        validate_theme_pack(pack, effects)?;
        if !ids.insert(pack.id.as_str()) {
            return Err(invalid(
                FILE,
                format!("theme pack {} declared twice", pack.id),
            ));
        }
    }
    Ok(())
}

fn validate_theme_pack(pack: &ThemePack, effects: &Effects) -> Result<(), CanonError> {
    const FILE: &str = "themes.toml";
    let id_ok = !pack.id.is_empty()
        && pack.id.len() <= THEME_ID_CHARS_MAX
        && pack
            .id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if !id_ok {
        return Err(invalid(FILE, format!("bad theme pack id {:?}", pack.id)));
    }
    if pack.label.is_empty() || pack.label.chars().count() > THEME_LABEL_CHARS_MAX {
        return Err(invalid(FILE, format!("pack {} has a bad label", pack.id)));
    }
    if pack.classes.is_empty() {
        return Err(invalid(FILE, format!("pack {} names no class", pack.id)));
    }
    let mut classes: BTreeSet<EffectClass> = BTreeSet::new();
    for class in &pack.classes {
        if !classes.insert(*class) {
            return Err(invalid(
                FILE,
                format!("pack {} names class {class} twice", pack.id),
            ));
        }
        // A class's own required grant is part of the pack's terms.
        if let Some(grant) = effects
            .class
            .iter()
            .find(|r| r.id == *class)
            .and_then(|r| r.requires_grant)
            && !pack.requires_grants.contains(&grant)
        {
            return Err(invalid(
                FILE,
                format!(
                    "pack {} omits the {grant:?} grant its class {class} requires",
                    pack.id
                ),
            ));
        }
    }
    let class_cap = pack
        .classes
        .iter()
        .map(|c| {
            effects
                .class
                .iter()
                .find(|r| r.id == *c)
                .map_or(INTENSITY_MAX, |r| r.max_intensity)
        })
        .min()
        .unwrap_or(INTENSITY_MAX);
    if !(INTENSITY_MIN..=class_cap).contains(&pack.max_intensity) {
        return Err(invalid(
            FILE,
            format!(
                "pack {} max_intensity {} exceeds its classes' bound {class_cap}",
                pack.id, pack.max_intensity
            ),
        ));
    }
    let mut grants: BTreeSet<Grant> = BTreeSet::new();
    for grant in &pack.requires_grants {
        if !grants.insert(*grant) {
            return Err(invalid(
                FILE,
                format!("pack {} names grant {grant:?} twice", pack.id),
            ));
        }
    }
    if let Some(sub) = &pack.subliminal {
        if !pack.requires_grants.contains(&Grant::Subliminal) {
            return Err(invalid(
                FILE,
                format!(
                    "pack {} carries a subliminal layer without its grant",
                    pack.id
                ),
            ));
        }
        if !sub.level_db.is_finite()
            || sub.level_db < SUBLIMINAL_LEVEL_DB_MIN
            || sub.level_db >= 0.0
        {
            return Err(invalid(
                FILE,
                format!(
                    "pack {} subliminal level_db {} is not a depth below the bed in [{SUBLIMINAL_LEVEL_DB_MIN}, 0)",
                    pack.id, sub.level_db
                ),
            ));
        }
        if !(spec::SSB_AM_CARRIER_MIN_HZ..=spec::SSB_AM_CARRIER_MAX_HZ).contains(&sub.carrier_hz) {
            return Err(invalid(
                FILE,
                format!(
                    "pack {} carrier {} outside the SSB-AM range",
                    pack.id, sub.carrier_hz
                ),
            ));
        }
    }
    if pack.programme_scoped
        && (!classes.contains(&EffectClass::AccumulationProgramme)
            || !pack.requires_grants.contains(&Grant::Longitudinal))
    {
        return Err(invalid(
            FILE,
            format!(
                "pack {} is programme-scoped without the accumulation class and longitudinal grant",
                pack.id
            ),
        ));
    }
    if pack.trigger_persistence {
        if !pack.requires_grants.contains(&Grant::TriggerPersistence) {
            return Err(invalid(
                FILE,
                format!("pack {} persists triggers without the grant", pack.id),
            ));
        }
        for class in &pack.classes {
            let record = effects.class.iter().find(|r| r.id == *class);
            if record.is_some_and(|r| r.scope == Scope::SessionOnly) {
                return Err(invalid(
                    FILE,
                    format!("pack {} persists session-only class {class}", pack.id),
                ));
            }
        }
    }
    if pack.anchor.is_some() && !classes.contains(&EffectClass::TriggerInstallation) {
        return Err(invalid(
            FILE,
            format!(
                "pack {} carries an anchor without the trigger class",
                pack.id
            ),
        ));
    }
    if let Some(anchor) = &pack.anchor {
        let fields = [
            ("cue", anchor.cue.as_str(), 40_usize),
            ("response", anchor.response.as_str(), 120),
            ("removal_procedure", anchor.removal_procedure.as_str(), 200),
        ];
        for (name, text, max) in fields {
            if text.is_empty() || text.chars().count() > max {
                return Err(invalid(
                    FILE,
                    format!("pack {} anchor {name} must be 1..={max} chars", pack.id),
                ));
            }
        }
    }
    validate_disclosure(pack)?;
    validate_affirmation_sets(pack)?;
    Ok(())
}

fn validate_disclosure(pack: &ThemePack) -> Result<(), CanonError> {
    const FILE: &str = "themes.toml";
    let d = &pack.disclosure;
    if d.summary.is_empty() || d.summary.chars().count() > DISCLOSURE_SUMMARY_CHARS_MAX {
        return Err(invalid(
            FILE,
            format!(
                "pack {} disclosure summary must be 1..={DISCLOSURE_SUMMARY_CHARS_MAX} chars",
                pack.id
            ),
        ));
    }
    if d.contents.is_empty() || d.contents.len() > DISCLOSURE_CONTENTS_MAX {
        return Err(invalid(
            FILE,
            format!(
                "pack {} disclosure contents must be 1..={DISCLOSURE_CONTENTS_MAX} items",
                pack.id
            ),
        ));
    }
    for item in &d.contents {
        if item.is_empty() || item.chars().count() > DISCLOSURE_CONTENT_CHARS_MAX {
            return Err(invalid(
                FILE,
                format!("pack {} has a bad disclosure content item", pack.id),
            ));
        }
    }
    // Structural, never waivable (PLAN §3.2, §3.6): the driving
    // flag and the stop cue ride every delivery.
    if !d.never_for_driving || !d.stop_cue_required {
        return Err(invalid(
            FILE,
            format!(
                "pack {} disclosure must set never_for_driving and stop_cue_required",
                pack.id
            ),
        ));
    }
    Ok(())
}

fn validate_affirmation_sets(pack: &ThemePack) -> Result<(), CanonError> {
    const FILE: &str = "themes.toml";
    if pack.affirmation_sets.is_empty()
        || pack.affirmation_sets.len() > AFFIRMATION_SETS_PER_PACK_MAX
    {
        return Err(invalid(
            FILE,
            format!(
                "pack {} must carry 1..={AFFIRMATION_SETS_PER_PACK_MAX} affirmation sets",
                pack.id
            ),
        ));
    }
    let mut set_ids: BTreeSet<&str> = BTreeSet::new();
    for set in &pack.affirmation_sets {
        if !set_ids.insert(set.id.as_str()) {
            return Err(invalid(
                FILE,
                format!("pack {} declares set {} twice", pack.id, set.id),
            ));
        }
        if set.version == 0 {
            return Err(invalid(
                FILE,
                format!("pack {} set {} has version 0", pack.id, set.id),
            ));
        }
        if set.cue_phrase.is_empty() || set.cue_phrase.chars().count() > CUE_PHRASE_CHARS_MAX {
            return Err(invalid(
                FILE,
                format!("pack {} set {} has a bad cue phrase", pack.id, set.id),
            ));
        }
        check_affirmation_text(pack, set, &set.cue_phrase)?;
        if set.lines.is_empty() || set.lines.len() > AFFIRMATION_LINES_PER_SET_MAX {
            return Err(invalid(
                FILE,
                format!(
                    "pack {} set {} must carry 1..={AFFIRMATION_LINES_PER_SET_MAX} lines",
                    pack.id, set.id
                ),
            ));
        }
        for line in &set.lines {
            if line.is_empty() || line.chars().count() > AFFIRMATION_LINE_CHARS_MAX {
                return Err(invalid(
                    FILE,
                    format!("pack {} set {} has a bad line", pack.id, set.id),
                ));
            }
            check_affirmation_text(pack, set, line)?;
        }
    }
    Ok(())
}

/// The §3.6 floor: no affirmation text targets the listener's
/// ability to refuse, stop, or remember the disclosure log.
fn check_affirmation_text(
    pack: &ThemePack,
    set: &AffirmationSet,
    text: &str,
) -> Result<(), CanonError> {
    let lower = text.to_lowercase();
    for phrase in FORBIDDEN_AFFIRMATION_PHRASES {
        if lower.contains(phrase) {
            return Err(invalid(
                "themes.toml",
                format!(
                    "pack {} set {} text targets the ability to stop or refuse ({phrase:?})",
                    pack.id, set.id
                ),
            ));
        }
    }
    Ok(())
}

fn validate_engagement(engagement: &Engagement) -> Result<(), CanonError> {
    const FILE: &str = "engagement.toml";
    if engagement.schema != 1 {
        return Err(invalid(
            FILE,
            format!("unsupported schema {}", engagement.schema),
        ));
    }
    let t = &engagement.trigger;
    if t.cue.is_empty() || t.cue.chars().count() > CUE_PHRASE_CHARS_MAX {
        return Err(invalid(FILE, "engagement cue must be 1..=60 chars"));
    }
    if t.prompt.is_empty() || t.prompt.chars().count() > DISCLOSURE_SUMMARY_CHARS_MAX {
        return Err(invalid(FILE, "engagement prompt must be 1..=400 chars"));
    }
    let fields: Vec<&str> = t.fields.iter().map(String::as_str).collect();
    if fields != ENGAGEMENT_FIELDS {
        return Err(invalid(
            FILE,
            format!("engagement fields must be exactly {ENGAGEMENT_FIELDS:?} in order"),
        ));
    }
    if !(1..=1_000_000).contains(&t.max_listen_count) {
        return Err(invalid(FILE, "max_listen_count must be 1..=1000000"));
    }
    let r = &engagement.replay_trigger;
    if r.cue.is_empty() || r.cue.chars().count() > CUE_PHRASE_CHARS_MAX {
        return Err(invalid(FILE, "replay cue must be 1..=60 chars"));
    }
    if r.prompt.is_empty() || r.prompt.chars().count() > DISCLOSURE_SUMMARY_CHARS_MAX {
        return Err(invalid(FILE, "replay prompt must be 1..=400 chars"));
    }
    if r.min_age != REPLAY_MIN_AGE {
        return Err(invalid(
            FILE,
            format!("replay min_age must be exactly {REPLAY_MIN_AGE} (hard gate)"),
        ));
    }
    if !r.requires_enjoyment {
        return Err(invalid(
            FILE,
            "replay trigger must require an enjoyment report",
        ));
    }
    Ok(())
}

fn load_blocks(sources: &[(&str, &str)]) -> Result<BTreeMap<BlockKind, BlockFile>, CanonError> {
    if sources.len() > BLOCK_FILES_MAX {
        return Err(invalid(
            "blocks/",
            format!("more than {BLOCK_FILES_MAX} block files"),
        ));
    }
    let mut blocks: BTreeMap<BlockKind, BlockFile> = BTreeMap::new();
    let mut template_ids: BTreeSet<String> = BTreeSet::new();
    for (name, text) in sources {
        let file: BlockFile = parse(name, text)?;
        validate_block(name, &file, &mut template_ids)?;
        if blocks.insert(file.kind, file).is_some() {
            return Err(invalid(name, "block kind declared by more than one file"));
        }
    }
    for kind in BlockKind::ALL {
        if !blocks.contains_key(&kind) {
            return Err(invalid(
                "blocks/",
                format!("block kind {kind:?} has no file"),
            ));
        }
    }
    let induction = &blocks[&BlockKind::Induction];
    for family in InductionFamily::ALL {
        if !induction.template.iter().any(|t| t.family == Some(family)) {
            return Err(invalid(
                "blocks/induction.toml",
                format!("family {family:?} missing"),
            ));
        }
    }
    Ok(blocks)
}

fn validate_block(
    name: &str,
    file: &BlockFile,
    template_ids: &mut BTreeSet<String>,
) -> Result<(), CanonError> {
    if file.schema != 1 {
        return Err(invalid(name, format!("unsupported schema {}", file.schema)));
    }
    if file.template.is_empty() {
        return Err(invalid(name, "block file declares no templates"));
    }
    for t in &file.template {
        if !template_ids.insert(t.id.clone()) {
            return Err(invalid(name, format!("template {} declared twice", t.id)));
        }
        validate_induction_fields(name, file.kind, t)?;
        validate_slots(name, t)?;
        if file.kind == BlockKind::SetupPermission
            && !t.slots.iter().any(|s| s.name() == "stop_cue")
        {
            return Err(invalid(
                name,
                format!("{}: set-up must carry a stop_cue slot", t.id),
            ));
        }
    }
    Ok(())
}

fn validate_induction_fields(
    name: &str,
    kind: BlockKind,
    t: &BlockTemplate,
) -> Result<(), CanonError> {
    if kind != BlockKind::Induction {
        if t.family.is_some() || t.live_context.is_some() {
            return Err(invalid(
                name,
                format!("{}: family/live_context are induction-only", t.id),
            ));
        }
        return Ok(());
    }
    let Some(family) = t.family else {
        return Err(invalid(
            name,
            format!("{}: induction template lacks family", t.id),
        ));
    };
    // Rapid induction, and only rapid, is flagged live-context.
    let flagged = t.live_context == Some(true);
    if flagged != (family == InductionFamily::Rapid) || t.live_context.is_none() {
        return Err(invalid(
            name,
            format!("{}: live_context must be true for rapid only", t.id),
        ));
    }
    Ok(())
}

/// Every `{slot}` in the text is declared, every declared slot is
/// used, names are unique, and numeric ranges are sane.
fn validate_slots(name: &str, t: &BlockTemplate) -> Result<(), CanonError> {
    if t.text.chars().count() > TEMPLATE_TEXT_CHARS_MAX {
        return Err(invalid(
            name,
            format!("{}: text exceeds {TEMPLATE_TEXT_CHARS_MAX} chars", t.id),
        ));
    }
    let mut declared: BTreeSet<&str> = BTreeSet::new();
    for slot in &t.slots {
        if !declared.insert(slot.name()) {
            return Err(invalid(
                name,
                format!("{}: slot {} declared twice", t.id, slot.name()),
            ));
        }
        let sane = match slot {
            SlotType::Text { max_chars, .. } => (1..=SLOT_TEXT_CHARS_MAX).contains(max_chars),
            SlotType::Count { min, max, .. } | SlotType::Seconds { min, max, .. } => {
                min <= max && *max <= SLOT_NUMBER_MAX
            }
        };
        if !sane {
            return Err(invalid(
                name,
                format!("{}: slot {} has a bad range", t.id, slot.name()),
            ));
        }
    }
    let used = placeholders(&t.text).map_err(|m| invalid(name, format!("{}: {m}", t.id)))?;
    if used != declared {
        return Err(invalid(
            name,
            format!("{}: text placeholders do not match declared slots", t.id),
        ));
    }
    Ok(())
}

/// The set of `{name}` placeholders in a template text.
fn placeholders(text: &str) -> Result<BTreeSet<&str>, String> {
    let mut out = BTreeSet::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        if rest[..open].contains('}') {
            return Err("stray closing brace".to_string());
        }
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            return Err("unclosed placeholder".to_string());
        };
        let slot = &after[..close];
        let well_formed = !slot.is_empty()
            && slot
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        if !well_formed {
            return Err(format!("malformed placeholder {{{slot}}}"));
        }
        out.insert(slot);
        rest = &after[close + 1..];
    }
    if rest.contains('}') {
        return Err("stray closing brace".to_string());
    }
    Ok(out)
}
