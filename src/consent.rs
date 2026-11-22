//! Consent contracts: the profile, its envelope, the programme
//! grant, and the typed session request (PLAN §3, §5).
//!
//! Every type here validates on construction or deserialization;
//! an invalid value cannot be held. Free text (exclusion themes,
//! request notes) is bounded data: it is stored and shown, never
//! parsed or judged — the gate reads typed fields only.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::canon::{EffectClass, Grant, INTENSITY_MAX, INTENSITY_MIN};
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
