//! Consent contracts: the profile, its envelope, the programme
//! grant, and the typed session request (PLAN §3, §5).
//!
//! Every type here validates on construction or deserialization;
//! an invalid value cannot be held. Free text (exclusion themes,
//! request notes) is bounded data: it is stored and shown, never
//! parsed or judged — the gate reads typed fields only.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::canon::{
    Canon, EffectClass, Grant, INTENSITY_MAX, INTENSITY_MIN, LayerRole, Scope, ThemePack,
};
use crate::error::{MesmerError, MesmerResult};

/// The plain statement of what use means (PLAN §3.1).
pub const CONSENT_STATEMENT: &str =
    "Using this is consenting to be hypnotized within the bounds recorded here.";
/// Profile record schema id.
pub const PROFILE_SCHEMA: u32 = 1;
/// Longest session a profile may permit, minutes.
pub const SESSION_MINUTES_MAX: u16 = 180;
/// Programme grant lifetime cap and default, days (PLAN §3.4).
pub const PROGRAMME_EXPIRY_DAYS_MAX: u32 = 28;
/// Highest cadence cap a programme grant may state, sessions/day.
pub const CADENCE_CAP_PER_DAY_MAX: u8 = 8;
/// Bounds on free-text data.
pub const EXCLUSIONS_MAX: usize = 32;
pub const EXCLUSION_CHARS_MAX: usize = 80;
pub const NOTES_CHARS_MAX: usize = 2000;
/// Profile id: 1..=64 chars of `[a-z0-9_-]` (safe as a file stem).
pub const PROFILE_ID_CHARS_MAX: usize = 64;

const SECONDS_PER_DAY: i64 = 86_400;

// ── Validated scalars ────────────────────────────────────────────

/// An intensity on the 1..=10 scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct Intensity(u8);

impl Intensity {
    pub fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for Intensity {
    type Error = String;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        if (INTENSITY_MIN..=INTENSITY_MAX).contains(&value) {
            Ok(Intensity(value))
        } else {
            Err(format!(
                "intensity {value} outside {INTENSITY_MIN}..={INTENSITY_MAX}"
            ))
        }
    }
}

impl From<Intensity> for u8 {
    fn from(value: Intensity) -> u8 {
        value.0
    }
}

/// A profile id, safe to use as a file stem.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ProfileId(String);

impl ProfileId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for ProfileId {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let charset_ok = value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
        if value.is_empty() || value.len() > PROFILE_ID_CHARS_MAX || !charset_ok {
            return Err("profile_id must be 1..=64 chars of [a-z0-9_-]".to_string());
        }
        Ok(ProfileId(value))
    }
}

impl From<ProfileId> for String {
    fn from(value: ProfileId) -> String {
        value.0
    }
}

/// Bounded free text. Data only: never parsed, never judged.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct FreeText<const MAX_CHARS: usize>(String);

impl<const MAX_CHARS: usize> FreeText<MAX_CHARS> {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<const MAX_CHARS: usize> TryFrom<String> for FreeText<MAX_CHARS> {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.chars().count() > MAX_CHARS {
            return Err(format!("text exceeds {MAX_CHARS} chars"));
        }
        Ok(FreeText(value))
    }
}

impl<const MAX_CHARS: usize> From<FreeText<MAX_CHARS>> for String {
    fn from(value: FreeText<MAX_CHARS>) -> String {
        value.0
    }
}

/// A user exclusion theme (on top of the canon's own exclusions).
pub type ExclusionTheme = FreeText<EXCLUSION_CHARS_MAX>;
/// Request notes.
pub type Notes = FreeText<NOTES_CHARS_MAX>;

// ── Grants ───────────────────────────────────────────────────────

/// The three separate, default-off grants.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Grants {
    pub subliminal: bool,
    pub trigger_persistence: bool,
    pub longitudinal: bool,
}

impl Grants {
    pub fn holds(&self, grant: Grant) -> bool {
        match grant {
            Grant::Subliminal => self.subliminal,
            Grant::TriggerPersistence => self.trigger_persistence,
            Grant::Longitudinal => self.longitudinal,
        }
    }
}

/// The longitudinal grant's terms (minimal for R2: expiry +
/// cadence cap; the programme engine is R4). Valid while
/// `now < expires_at_unix_s`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgrammeGrant {
    pub expires_at_unix_s: i64,
    pub cadence_cap_per_day: u8,
}

impl ProgrammeGrant {
    pub fn is_expired(&self, now_unix_s: i64) -> bool {
        now_unix_s >= self.expires_at_unix_s
    }
}

/// Programme terms as the user states them (relative expiry).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgrammeTerms {
    #[serde(default = "default_expiry_days")]
    pub expires_in_days: u32,
    pub cadence_cap_per_day: u8,
}

fn default_expiry_days() -> u32 {
    PROGRAMME_EXPIRY_DAYS_MAX
}

impl ProgrammeTerms {
    /// Resolve to an absolute grant at `now_unix_s`.
    pub fn resolve(self, now_unix_s: i64) -> MesmerResult<ProgrammeGrant> {
        if !(1..=PROGRAMME_EXPIRY_DAYS_MAX).contains(&self.expires_in_days) {
            return Err(MesmerError::invalid_input("expires_in_days must be 1..=28"));
        }
        if !(1..=CADENCE_CAP_PER_DAY_MAX).contains(&self.cadence_cap_per_day) {
            return Err(MesmerError::invalid_input(
                "cadence_cap_per_day must be 1..=8",
            ));
        }
        let lifetime_s = i64::from(self.expires_in_days) * SECONDS_PER_DAY;
        let expires_at_unix_s = now_unix_s
            .checked_add(lifetime_s)
            .ok_or_else(|| MesmerError::invalid_input("expiry overflows the clock"))?;
        Ok(ProgrammeGrant {
            expires_at_unix_s,
            cadence_cap_per_day: self.cadence_cap_per_day,
        })
    }
}

// ── The profile ──────────────────────────────────────────────────

/// The consent profile: the consent act, as a typed record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentProfile {
    pub schema: u32,
    pub profile_id: ProfileId,
    pub revision: u32,
    pub created_at_unix_s: i64,
    pub amended_at_unix_s: i64,
    pub adult_attested: bool,
    pub allowed_classes: BTreeSet<EffectClass>,
    pub exclusions: BTreeSet<ExclusionTheme>,
    pub intensity_ceiling: Intensity,
    pub max_session_minutes: u16,
    pub grants: Grants,
    pub programme: Option<ProgrammeGrant>,
}

/// What a user submits to create a profile.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileDraft {
    pub adult_attested: bool,
    pub allowed_classes: BTreeSet<EffectClass>,
    #[serde(default)]
    pub exclusions: BTreeSet<ExclusionTheme>,
    pub intensity_ceiling: Intensity,
    pub max_session_minutes: u16,
    #[serde(default)]
    pub grants: Grants,
    #[serde(default)]
    pub programme: Option<ProgrammeTerms>,
}

/// A partial amendment. Absent fields are unchanged.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Amendment {
    pub allowed_classes: Option<BTreeSet<EffectClass>>,
    pub exclusions: Option<BTreeSet<ExclusionTheme>>,
    pub intensity_ceiling: Option<Intensity>,
    pub max_session_minutes: Option<u16>,
    pub grants: Option<GrantsPatch>,
    pub programme: Option<ProgrammeTerms>,
}

/// Per-grant patch; absent grants are unchanged.
#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantsPatch {
    pub subliminal: Option<bool>,
    pub trigger_persistence: Option<bool>,
    pub longitudinal: Option<bool>,
}

impl ConsentProfile {
    /// Build a new profile from a draft. Validates every field;
    /// refuses without the adult attestation.
    pub fn create(id: ProfileId, draft: ProfileDraft, now_unix_s: i64) -> MesmerResult<Self> {
        let programme = match draft.programme {
            Some(terms) => Some(terms.resolve(now_unix_s)?),
            None => None,
        };
        let profile = ConsentProfile {
            schema: PROFILE_SCHEMA,
            profile_id: id,
            revision: 1,
            created_at_unix_s: now_unix_s,
            amended_at_unix_s: now_unix_s,
            adult_attested: draft.adult_attested,
            allowed_classes: draft.allowed_classes,
            exclusions: draft.exclusions,
            intensity_ceiling: draft.intensity_ceiling,
            max_session_minutes: draft.max_session_minutes,
            grants: draft.grants,
            programme,
        };
        profile.validate()?;
        Ok(profile)
    }

    /// The structural invariants every stored profile satisfies.
    pub fn validate(&self) -> MesmerResult<()> {
        if self.schema != PROFILE_SCHEMA {
            return Err(MesmerError::invalid_input("unsupported profile schema"));
        }
        if !self.adult_attested {
            return Err(MesmerError::invalid_input(
                "adult_attested must be true: mesmer serves consenting adults only",
            ));
        }
        if !(1..=SESSION_MINUTES_MAX).contains(&self.max_session_minutes) {
            return Err(MesmerError::invalid_input(
                "max_session_minutes must be 1..=180",
            ));
        }
        if self.exclusions.len() > EXCLUSIONS_MAX {
            return Err(MesmerError::invalid_input("at most 32 exclusions"));
        }
        if self.grants.longitudinal != self.programme.is_some() {
            return Err(MesmerError::invalid_input(
                "the longitudinal grant and its programme terms come together: \
                 grant longitudinal with programme {cadence_cap_per_day, expires_in_days}",
            ));
        }
        Ok(())
    }

    /// The candidate profile an amendment would produce. Pure:
    /// whether the candidate may be committed is the gate's call.
    pub fn amended(&self, amendment: &Amendment, now_unix_s: i64) -> MesmerResult<Self> {
        let mut next = self.clone();
        if let Some(classes) = &amendment.allowed_classes {
            next.allowed_classes = classes.clone();
        }
        if let Some(exclusions) = &amendment.exclusions {
            next.exclusions = exclusions.clone();
        }
        if let Some(ceiling) = amendment.intensity_ceiling {
            next.intensity_ceiling = ceiling;
        }
        if let Some(minutes) = amendment.max_session_minutes {
            next.max_session_minutes = minutes;
        }
        if let Some(patch) = amendment.grants {
            next.grants.subliminal = patch.subliminal.unwrap_or(next.grants.subliminal);
            next.grants.trigger_persistence = patch
                .trigger_persistence
                .unwrap_or(next.grants.trigger_persistence);
            next.grants.longitudinal = patch.longitudinal.unwrap_or(next.grants.longitudinal);
        }
        if let Some(terms) = amendment.programme {
            next.programme = Some(terms.resolve(now_unix_s)?);
        }
        // Revoking the longitudinal grant closes its programme terms.
        if !next.grants.longitudinal && amendment.programme.is_none() {
            next.programme = None;
        }
        next.revision = self
            .revision
            .checked_add(1)
            .ok_or_else(|| MesmerError::invalid_input("profile revision overflow"))?;
        next.amended_at_unix_s = now_unix_s;
        next.validate()?;
        Ok(next)
    }

    pub fn envelope(&self) -> Envelope {
        Envelope::from(self)
    }
}

// ── The envelope ─────────────────────────────────────────────────

/// What consent attaches to (PLAN §3.2): the bounds derived from a
/// profile. Payloads inside the envelope need not be previewed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Envelope {
    pub allowed_classes: BTreeSet<EffectClass>,
    pub exclusions: BTreeSet<ExclusionTheme>,
    pub intensity_ceiling: Intensity,
    pub max_session_minutes: u16,
    pub grants: Grants,
    pub programme: Option<ProgrammeGrant>,
}

impl From<&ConsentProfile> for Envelope {
    fn from(p: &ConsentProfile) -> Envelope {
        Envelope {
            allowed_classes: p.allowed_classes.clone(),
            exclusions: p.exclusions.clone(),
            intensity_ceiling: p.intensity_ceiling,
            max_session_minutes: p.max_session_minutes,
            grants: p.grants,
            programme: p.programme,
        }
    }
}

impl Envelope {
    /// Every dimension along which `candidate` is wider than
    /// `self`. Empty means the candidate is within this envelope
    /// (a narrowing or no change). Removing an exclusion widens.
    pub fn widenings(&self, candidate: &Envelope) -> Vec<String> {
        let mut out = Vec::new();
        for class in candidate.allowed_classes.difference(&self.allowed_classes) {
            out.push(format!("adds class {class}"));
        }
        for theme in self.exclusions.difference(&candidate.exclusions) {
            out.push(format!("removes exclusion \"{}\"", theme.as_str()));
        }
        if candidate.intensity_ceiling > self.intensity_ceiling {
            out.push("raises intensity_ceiling".to_string());
        }
        if candidate.max_session_minutes > self.max_session_minutes {
            out.push("raises max_session_minutes".to_string());
        }
        let grants = [
            Grant::Subliminal,
            Grant::TriggerPersistence,
            Grant::Longitudinal,
        ];
        for grant in grants {
            if candidate.grants.holds(grant) && !self.grants.holds(grant) {
                out.push(format!("grants {grant:?}"));
            }
        }
        if let (Some(old), Some(new)) = (self.programme, candidate.programme) {
            if new.expires_at_unix_s > old.expires_at_unix_s {
                out.push("extends programme expiry".to_string());
            }
            if new.cadence_cap_per_day > old.cadence_cap_per_day {
                out.push("raises programme cadence cap".to_string());
            }
        }
        out
    }
}

// ── The request and its context ──────────────────────────────────

/// A subliminal layer, specified by depth below the bed only.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawSubliminalLayer")]
pub struct SubliminalLayer {
    pub level_db: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSubliminalLayer {
    level_db: f64,
}

impl TryFrom<RawSubliminalLayer> for SubliminalLayer {
    type Error = String;
    fn try_from(raw: RawSubliminalLayer) -> Result<Self, Self::Error> {
        if !raw.level_db.is_finite() || raw.level_db >= 0.0 {
            return Err(
                "subliminal level_db is depth below the bed: must be finite and < 0".into(),
            );
        }
        Ok(SubliminalLayer {
            level_db: raw.level_db,
        })
    }
}

/// A generation request, typed. `notes` is data: it is never
/// parsed, and the gate never reads it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRequest {
    pub classes: BTreeSet<EffectClass>,
    pub intensity: Intensity,
    pub duration_minutes: u16,
    #[serde(default)]
    pub subliminal_layer: Option<SubliminalLayer>,
    #[serde(default)]
    pub trigger_persistence: bool,
    #[serde(default)]
    pub programme_scoped: bool,
    #[serde(default)]
    pub notes: Option<Notes>,
}

/// The gate's view of the moment: the time (an input — the gate
/// never reads a clock) and whether a session is active.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionContext {
    pub now_unix_s: i64,
    pub session_active: bool,
}

// ── Playlists and composites (1.1.0, playlists SPEC §4.2) ───────

/// Playlist record schema id.
pub const PLAYLIST_SCHEMA: u32 = 1;
/// Playlist ids follow the profile-id charset rules (SPEC §4.2).
pub type PlaylistId = ProfileId;
/// Longest playlist title, chars. Titles are data: stored and
/// shown, never judged.
pub const PLAYLIST_TITLE_CHARS_MAX: usize = 120;
/// A playlist title.
pub type PlaylistTitle = FreeText<PLAYLIST_TITLE_CHARS_MAX>;
/// Most anchor cues one build/end-state spec may name.
pub const ANCHOR_CUES_PER_SPEC_MAX: usize = 8;
/// Longest anchor cue, chars (canon bounds a pack's cue at 40).
pub const ANCHOR_CUE_CHARS_MAX: usize = 40;

/// The request a theme pack makes when delivered at one
/// intensity for one duration: the pack's own classes, subliminal
/// layer, and persistence terms — the derivation
/// `theme_pack_deliver` has always performed, named once here so
/// entries and composites derive requests identically.
pub fn pack_request(
    pack: &ThemePack,
    intensity: Intensity,
    duration_minutes: u16,
) -> SessionRequest {
    SessionRequest {
        classes: pack.classes.iter().copied().collect(),
        intensity,
        duration_minutes,
        subliminal_layer: pack.subliminal.map(|s| SubliminalLayer {
            level_db: s.level_db,
        }),
        trigger_persistence: pack.trigger_persistence,
        programme_scoped: pack.programme_scoped,
        notes: None,
    }
}

/// Whether an end state is claimed for the run's arc only, or is
/// meant to persist and accumulate under the programme regime
/// (SPEC §2.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndStatePersistence {
    RunOnly,
    Programme,
}

/// A playlist's declared desired end state (SPEC §2.3): taxonomy
/// classes, a depth target on the intensity scale, a persistence
/// term, and the anchor cues expected installed by the final
/// entry. Typed only — there is no free-text end state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndState {
    pub classes: BTreeSet<EffectClass>,
    pub depth_target: Intensity,
    pub persistence: EndStatePersistence,
    #[serde(default)]
    pub anchors: Vec<String>,
}

/// One entry's compounding declaration (SPEC §2.2): strictly
/// backward build references, a depth target, and anchor cues
/// installed by earlier entries of the same playlist.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildSpec {
    #[serde(default)]
    pub builds_on: Vec<u16>,
    pub depth_target: Intensity,
    #[serde(default)]
    pub anchors: Vec<String>,
}

/// One composite layer: a role and the pack that fills it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayerSpec {
    pub role: LayerRole,
    pub pack_id: String,
}

/// A simultaneous composite delivery (SPEC §3.2): layered packs
/// under one intensity and one duration, judged once as one
/// delivery.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompositeSpec {
    pub intensity: Intensity,
    pub duration_minutes: u16,
    pub layers: Vec<LayerSpec>,
}

impl CompositeSpec {
    /// The pack filling one role, if the role is present.
    pub fn layer_pack<'c>(&self, canon: &'c Canon, role: LayerRole) -> Option<&'c ThemePack> {
        self.layers
            .iter()
            .find(|l| l.role == role)
            .and_then(|l| canon.theme_pack(&l.pack_id))
    }

    /// The composite's single request (SPEC §3.3, composition
    /// pass): the union of its layers' classes, its subliminal
    /// layer's spec (the bed's own, when no subliminal layer is
    /// named), and the OR of its layers' persistence terms.
    pub fn request(&self, canon: &Canon) -> SessionRequest {
        let mut classes = BTreeSet::new();
        let mut trigger_persistence = false;
        let mut programme_scoped = false;
        for layer in &self.layers {
            if let Some(pack) = canon.theme_pack(&layer.pack_id) {
                classes.extend(pack.classes.iter().copied());
                trigger_persistence |= pack.trigger_persistence;
                programme_scoped |= pack.programme_scoped;
            }
        }
        let subliminal_layer = [LayerRole::Subliminal, LayerRole::Bed]
            .into_iter()
            .filter_map(|role| self.layer_pack(canon, role))
            .find_map(|pack| pack.subliminal)
            .map(|s| SubliminalLayer {
                level_db: s.level_db,
            });
        SessionRequest {
            classes,
            intensity: self.intensity,
            duration_minutes: self.duration_minutes,
            subliminal_layer,
            trigger_persistence,
            programme_scoped,
            notes: None,
        }
    }

    /// Structural validation against canon (SPEC §3.2): bounds,
    /// role counts, pack existence, and the anchor's one seat.
    /// Failures are `MESMER-PLAYLIST-INVALID`: a composite that
    /// fails validation does not exist as a delivery.
    pub fn validate(&self, canon: &Canon) -> MesmerResult<()> {
        let invalid = |detail: String| MesmerError::new("MESMER-PLAYLIST-INVALID", detail);
        if self.duration_minutes == 0 {
            return Err(invalid(
                "composite duration_minutes must be >= 1".to_string(),
            ));
        }
        let max = canon.playlists.max_layers_per_composite;
        if self.layers.is_empty() || self.layers.len() > max {
            return Err(invalid(format!(
                "a composite carries 1..={max} layers, not {}",
                self.layers.len()
            )));
        }
        let count = |role: LayerRole| self.layers.iter().filter(|l| l.role == role).count();
        if count(LayerRole::Bed) != 1 {
            return Err(invalid(
                "a composite carries exactly one bed layer".to_string(),
            ));
        }
        if count(LayerRole::Subliminal) > 1 {
            return Err(invalid(
                "a composite carries at most one subliminal layer".to_string(),
            ));
        }
        if count(LayerRole::Visual) > 1 {
            return Err(invalid(
                "a composite carries at most one visual layer".to_string(),
            ));
        }
        for layer in &self.layers {
            let Some(pack) = canon.theme_pack(&layer.pack_id) else {
                return Err(invalid(format!(
                    "no theme pack {:?} for the {} layer",
                    layer.pack_id, layer.role
                )));
            };
            if layer.role == LayerRole::Subliminal && pack.subliminal.is_none() {
                return Err(invalid(format!(
                    "pack {} carries no subliminal layer to fill the subliminal role",
                    pack.id
                )));
            }
            if layer.role != LayerRole::Bed && pack.anchor.is_some() {
                return Err(invalid(format!(
                    "pack {} carries an anchor; an anchor may ride only the bed layer",
                    pack.id
                )));
            }
        }
        Ok(())
    }
}

/// What one entry delivers: a theme pack, or a composite.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EntryDelivery {
    Pack { pack_id: String },
    Composite { composite: CompositeSpec },
}

/// One playlist entry (SPEC §4.2). For a composite entry the
/// composite's own intensity/duration govern and the entry-level
/// pair must agree with them (validation refuses a disagreement:
/// an entry states its terms once).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaylistEntry {
    pub delivery: EntryDelivery,
    pub intensity: Intensity,
    pub duration_minutes: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<BuildSpec>,
}

/// What a user submits to author a playlist (mirrors
/// [`ProfileDraft`]): the definition whole, validated
/// server-side. The server stamps schema, revision, and time.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaylistDraft {
    pub playlist_id: PlaylistId,
    pub entries: Vec<PlaylistEntry>,
    pub end_state: EndState,
    #[serde(default)]
    pub title: Option<PlaylistTitle>,
}

/// A stored playlist: a typed, immutable artefact bound to one
/// consent profile (SPEC §1.1). Changing one means deleting it
/// and creating its successor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaylistDefinition {
    pub schema: u32,
    pub playlist_id: PlaylistId,
    pub profile_id: ProfileId,
    pub revision: u32,
    pub created_at_unix_s: i64,
    pub entries: Vec<PlaylistEntry>,
    pub end_state: EndState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<PlaylistTitle>,
}

impl PlaylistDefinition {
    /// Build a stored definition from a draft. Canon-dependent
    /// validation is [`PlaylistDefinition::validate`].
    pub fn create(id: ProfileId, draft: PlaylistDraft, now_unix_s: i64) -> MesmerResult<Self> {
        Ok(PlaylistDefinition {
            schema: PLAYLIST_SCHEMA,
            playlist_id: draft.playlist_id,
            profile_id: id,
            revision: 1,
            created_at_unix_s: now_unix_s,
            entries: draft.entries,
            end_state: draft.end_state,
            title: draft.title,
        })
    }

    /// The classes one entry delivers, derived from canon.
    pub fn entry_classes(&self, canon: &Canon, index: usize) -> BTreeSet<EffectClass> {
        match &self.entries[index].delivery {
            EntryDelivery::Pack { pack_id } => canon
                .theme_pack(pack_id)
                .map(|p| p.classes.iter().copied().collect())
                .unwrap_or_default(),
            EntryDelivery::Composite { composite } => composite.request(canon).classes,
        }
    }

    /// The anchor cue one entry installs, if any: a pack's own
    /// anchor, or a composite's bed layer's anchor.
    pub fn entry_anchor_cue(&self, canon: &Canon, index: usize) -> Option<String> {
        match &self.entries[index].delivery {
            EntryDelivery::Pack { pack_id } => canon
                .theme_pack(pack_id)
                .and_then(|p| p.anchor.as_ref().map(|a| a.cue.clone())),
            EntryDelivery::Composite { composite } => composite
                .layer_pack(canon, LayerRole::Bed)
                .and_then(|p| p.anchor.as_ref().map(|a| a.cue.clone())),
        }
    }

    /// Whether one entry's delivery persists its anchor
    /// installation (its delivery carries trigger persistence).
    pub fn entry_persists(&self, canon: &Canon, index: usize) -> bool {
        match &self.entries[index].delivery {
            EntryDelivery::Pack { pack_id } => canon
                .theme_pack(pack_id)
                .is_some_and(|p| p.trigger_persistence),
            EntryDelivery::Composite { composite } => composite.request(canon).trigger_persistence,
        }
    }

    /// The request one entry will be judged by (SPEC §1.2): the
    /// delivery's own request, programme-scoped when the
    /// playlist's end state persists (SPEC §2.2 rule 5).
    pub fn entry_request(&self, canon: &Canon, index: usize) -> SessionRequest {
        let entry = &self.entries[index];
        let mut request = match &entry.delivery {
            EntryDelivery::Pack { pack_id } => {
                let pack = canon
                    .theme_pack(pack_id)
                    .expect("validated playlist names only canon packs");
                pack_request(pack, entry.intensity, entry.duration_minutes)
            }
            EntryDelivery::Composite { composite } => composite.request(canon),
        };
        if self.end_state.persistence == EndStatePersistence::Programme {
            request.programme_scoped = true;
        }
        request
    }

    /// Full structural validation against canon (SPEC §2.1,
    /// §2.2): bounds, pack existence, the compounding rules, and
    /// end-state reachability as authored. Profile-dependent
    /// judgement is the gate's (`gate::judge_playlist`). Errors
    /// carry the feature's codes: structural failures are
    /// `MESMER-PLAYLIST-INVALID`; a compounding or persistence
    /// claim on session-only scope is `MESMER-PLAYLIST-SCOPE`.
    pub fn validate(&self, canon: &Canon) -> MesmerResult<()> {
        let invalid = |detail: String| MesmerError::new("MESMER-PLAYLIST-INVALID", detail);
        let scope_err = |detail: String| MesmerError::new("MESMER-PLAYLIST-SCOPE", detail);
        if self.schema != PLAYLIST_SCHEMA {
            return Err(invalid(format!(
                "unsupported playlist schema {}",
                self.schema
            )));
        }
        let max_entries = canon.playlists.max_entries_per_playlist;
        if self.entries.is_empty() || self.entries.len() > max_entries {
            return Err(invalid(format!(
                "a playlist carries 1..={max_entries} entries, not {}",
                self.entries.len()
            )));
        }
        for (i, entry) in self.entries.iter().enumerate() {
            if entry.duration_minutes == 0 {
                return Err(invalid(format!("entry {i} duration_minutes must be >= 1")));
            }
            match &entry.delivery {
                EntryDelivery::Pack { pack_id } => {
                    if canon.theme_pack(pack_id).is_none() {
                        return Err(invalid(format!(
                            "entry {i} names no theme pack {pack_id:?}"
                        )));
                    }
                }
                EntryDelivery::Composite { composite } => {
                    composite.validate(canon)?;
                    if entry.intensity != composite.intensity
                        || entry.duration_minutes != composite.duration_minutes
                    {
                        return Err(invalid(format!(
                            "entry {i} states its intensity and duration twice and they differ"
                        )));
                    }
                }
            }
        }
        // Compounding (SPEC §2.2): backward references only,
        // monotonic depth in canon-bounded steps, scope honesty,
        // anchor continuity — all within this playlist alone.
        let mut previous_target: Option<u8> = None;
        let mut installed: Vec<(usize, String)> = Vec::new();
        for (i, entry) in self.entries.iter().enumerate() {
            let classes = self.entry_classes(canon, i);
            // Scope honesty (SPEC §2.2): an entry whose delivery
            // claims persistence while naming any session-only
            // class may not compound — the same dishonesty the
            // gate's class check refuses at delivery time, here
            // refused at authoring with the playlist scope code.
            let request = self.entry_request(canon, i);
            let scope_dishonest = request.trigger_persistence
                && classes
                    .iter()
                    .any(|c| canon.class(*c).scope == Scope::SessionOnly);
            if let Some(build) = &entry.build {
                let mut seen: BTreeSet<u16> = BTreeSet::new();
                for &back in &build.builds_on {
                    if !seen.insert(back) {
                        return Err(invalid(format!("entry {i} builds on entry {back} twice")));
                    }
                    if usize::from(back) >= i {
                        return Err(invalid(format!(
                            "entry {i} builds on entry {back}: references are backward only"
                        )));
                    }
                    let back_classes = self.entry_classes(canon, usize::from(back));
                    if !back_classes.is_empty()
                        && back_classes
                            .iter()
                            .all(|c| canon.class(*c).scope == Scope::SessionOnly)
                    {
                        return Err(scope_err(format!(
                            "entry {i} builds on entry {back}, whose classes are session-only: nothing of it carries"
                        )));
                    }
                }
                let target = build.depth_target.get();
                if let Some(prev) = previous_target {
                    if target < prev {
                        return Err(invalid(format!(
                            "entry {i} depth_target {target} falls below the previous target {prev}: depth is monotonic"
                        )));
                    }
                    if target - prev > canon.playlists.max_depth_step {
                        return Err(invalid(format!(
                            "entry {i} depth step {} exceeds the canon bound {}",
                            target - prev,
                            canon.playlists.max_depth_step
                        )));
                    }
                }
                let raised = target > previous_target.unwrap_or(INTENSITY_MIN);
                let claims = !build.builds_on.is_empty() || !build.anchors.is_empty() || raised;
                if claims && scope_dishonest {
                    return Err(scope_err(format!(
                        "entry {i} declares a build while claiming persistence over a session-only class: nothing of it may persist"
                    )));
                }
                if build.anchors.len() > ANCHOR_CUES_PER_SPEC_MAX {
                    return Err(invalid(format!(
                        "entry {i} names more than {ANCHOR_CUES_PER_SPEC_MAX} anchor cues"
                    )));
                }
                for cue in &build.anchors {
                    check_anchor_cue(cue)?;
                    let Some(&(installer, _)) = installed.iter().find(|(_, c)| c == cue) else {
                        return Err(invalid(format!(
                            "entry {i} names anchor {cue:?}, which no earlier entry installs"
                        )));
                    };
                    if !self.entry_persists(canon, installer) {
                        return Err(scope_err(format!(
                            "entry {i} names anchor {cue:?}, whose installation does not persist"
                        )));
                    }
                }
                // Compounding is a property of the sequence
                // (SPEC §2.2 rule 4): an entry may build only
                // on entries whose delivery persists.
                for &back in &build.builds_on {
                    if !self.entry_persists(canon, usize::from(back)) {
                        return Err(scope_err(format!(
                            "entry {i} builds on entry {back}, whose delivery does not persist"
                        )));
                    }
                }
                previous_target = Some(target);
            }
            if let Some(cue) = self.entry_anchor_cue(canon, i) {
                installed.push((i, cue));
            }
        }
        // The end state, as authored (SPEC §2.2 rule 2, §2.4).
        if self.end_state.classes.is_empty() {
            return Err(invalid("the end state names no class".to_string()));
        }
        let delivered: BTreeSet<EffectClass> = (0..self.entries.len())
            .flat_map(|i| self.entry_classes(canon, i))
            .collect();
        for class in &self.end_state.classes {
            if !delivered.contains(class) {
                return Err(invalid(format!(
                    "end state class {class} is delivered by no entry"
                )));
            }
        }
        if self.end_state.anchors.len() > ANCHOR_CUES_PER_SPEC_MAX {
            return Err(invalid(format!(
                "the end state names more than {ANCHOR_CUES_PER_SPEC_MAX} anchor cues"
            )));
        }
        for cue in &self.end_state.anchors {
            check_anchor_cue(cue)?;
            if !installed.iter().any(|(_, c)| c == cue) {
                return Err(invalid(format!(
                    "end state anchor {cue:?} is installed by no entry"
                )));
            }
        }
        let Some(final_build) = &self.entries.last().and_then(|e| e.build.as_ref()) else {
            return Err(invalid(
                "the final entry must declare the end state's depth target".to_string(),
            ));
        };
        if final_build.depth_target != self.end_state.depth_target {
            return Err(invalid(format!(
                "the final entry's depth_target {} is not the end state's {}",
                final_build.depth_target.get(),
                self.end_state.depth_target.get()
            )));
        }
        Ok(())
    }
}

fn check_anchor_cue(cue: &str) -> MesmerResult<()> {
    if cue.is_empty() || cue.chars().count() > ANCHOR_CUE_CHARS_MAX {
        return Err(MesmerError::new(
            "MESMER-PLAYLIST-INVALID",
            format!("anchor cue must be 1..={ANCHOR_CUE_CHARS_MAX} chars"),
        ));
    }
    Ok(())
}
