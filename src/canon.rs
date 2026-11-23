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
    pub blocks: Vec<(&'a str, &'a str)>,
}

impl CanonSources<'static> {
    /// The canon shipped in this repo, embedded at compile time.
    pub fn embedded() -> CanonSources<'static> {
        CanonSources {
            effects: include_str!("../canon/effects.toml"),
            patterns: include_str!("../canon/patterns.toml"),
            encoding: include_str!("../canon/encoding.toml"),
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
        let blocks = load_blocks(&sources.blocks)?;
        Ok(Canon {
            effects,
            patterns,
            encoding,
            blocks,
        })
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
