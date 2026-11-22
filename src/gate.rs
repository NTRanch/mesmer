//! The consent gate (PLAN §3): pure functions, no I/O, no clock.
//!
//! [`decide`] judges a typed [`SessionRequest`] against a profile's
//! envelope and returns [`Decision::Allow`] or every applicable
//! typed refusal, in fixed rule order. It reads typed fields only;
//! free text never reaches a verdict. [`decide_amend`] and
//! [`decide_create`] gate the profile lifecycle: narrowing is
//! always allowed, widening is refused while a session is active.
//! Revocation has no gate: it is total and always available.

use serde::Serialize;

use crate::canon::{Canon, Grant, Scope};
use crate::consent::{ConsentProfile, SessionContext, SessionRequest};

/// The refusal taxonomy (fleet convention: stable `MESMER-*` codes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum RefusalCode {
    #[serde(rename = "MESMER-NO-PROFILE")]
    NoProfile,
    #[serde(rename = "MESMER-ENVELOPE-CLASS")]
    EnvelopeClass,
    #[serde(rename = "MESMER-GRANT-ABSENT")]
    GrantAbsent,
    #[serde(rename = "MESMER-CEILING")]
    Ceiling,
    #[serde(rename = "MESMER-PROGRAMME-EXPIRED")]
    ProgrammeExpired,
    #[serde(rename = "MESMER-WIDEN-IN-SESSION")]
    WidenInSession,
}

impl RefusalCode {
    pub fn as_str(self) -> &'static str {
        match self {
            RefusalCode::NoProfile => "MESMER-NO-PROFILE",
            RefusalCode::EnvelopeClass => "MESMER-ENVELOPE-CLASS",
            RefusalCode::GrantAbsent => "MESMER-GRANT-ABSENT",
            RefusalCode::Ceiling => "MESMER-CEILING",
            RefusalCode::ProgrammeExpired => "MESMER-PROGRAMME-EXPIRED",
            RefusalCode::WidenInSession => "MESMER-WIDEN-IN-SESSION",
        }
    }
}

/// One typed refusal with a bounded, human-readable detail.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Refusal {
    pub code: RefusalCode,
    pub detail: String,
}

impl Refusal {
    fn new(code: RefusalCode, detail: impl Into<String>) -> Refusal {
        Refusal {
            code,
            detail: detail.into(),
        }
    }
}

/// The gate's verdict. `Refuse` is never empty.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "verdict", content = "refusals", rename_all = "snake_case")]
pub enum Decision {
    Allow,
    Refuse(Vec<Refusal>),
}

impl Decision {
    pub fn is_allow(&self) -> bool {
        matches!(self, Decision::Allow)
    }

    /// Refusal codes in rule order (empty for Allow).
    pub fn codes(&self) -> Vec<RefusalCode> {
        match self {
            Decision::Allow => Vec::new(),
            Decision::Refuse(refusals) => refusals.iter().map(|r| r.code).collect(),
        }
    }
}

/// Judge a generation request. Pure: same inputs, same verdict.
///
/// Rule order: no profile (alone, terminal) → envelope class →
/// grants → ceilings → programme expiry. Every applicable refusal
/// is reported so a preview can say everything that is wrong.
pub fn decide(
    canon: &Canon,
    profile: Option<&ConsentProfile>,
    context: &SessionContext,
    request: &SessionRequest,
) -> Decision {
    let Some(profile) = profile else {
        return Decision::Refuse(vec![Refusal::new(
            RefusalCode::NoProfile,
            "no consent profile: nothing is allowed",
        )]);
    };
    let mut refusals = Vec::new();
    check_classes(canon, profile, request, &mut refusals);
    check_grants(canon, profile, request, &mut refusals);
    check_ceilings(canon, profile, request, &mut refusals);
    check_programme(profile, context, request, &mut refusals);
    if refusals.is_empty() {
        Decision::Allow
    } else {
        Decision::Refuse(refusals)
    }
}

fn check_classes(
    canon: &Canon,
    profile: &ConsentProfile,
    request: &SessionRequest,
    out: &mut Vec<Refusal>,
) {
    if request.classes.is_empty() {
        out.push(Refusal::new(
            RefusalCode::EnvelopeClass,
            "request names no class",
        ));
    }
    for class in &request.classes {
        if !profile.allowed_classes.contains(class) {
            out.push(Refusal::new(
                RefusalCode::EnvelopeClass,
                format!("class {class} is outside the envelope"),
            ));
        }
    }
    if request.trigger_persistence {
        for class in &request.classes {
            if canon.class(*class).scope == Scope::SessionOnly {
                out.push(Refusal::new(
                    RefusalCode::EnvelopeClass,
                    format!("class {class} is session-scoped only; nothing of it may persist"),
                ));
            }
        }
    }
}

fn check_grants(
    canon: &Canon,
    profile: &ConsentProfile,
    request: &SessionRequest,
    out: &mut Vec<Refusal>,
) {
    let mut needed: Vec<(Grant, String)> = Vec::new();
    if request.subliminal_layer.is_some() {
        needed.push((Grant::Subliminal, "a subliminal layer".to_string()));
    }
    if request.trigger_persistence {
        needed.push((Grant::TriggerPersistence, "trigger persistence".to_string()));
    }
    if request.programme_scoped {
        needed.push((Grant::Longitudinal, "programme scope".to_string()));
    }
    for class in &request.classes {
        if let Some(grant) = canon.class(*class).requires_grant {
            needed.push((grant, format!("class {class}")));
        }
    }
    for (grant, what) in needed {
        if !profile.grants.holds(grant) {
            out.push(Refusal::new(
                RefusalCode::GrantAbsent,
                format!("{what} needs the {grant:?} grant, which the profile does not hold"),
            ));
        }
    }
}

fn check_ceilings(
    canon: &Canon,
    profile: &ConsentProfile,
    request: &SessionRequest,
    out: &mut Vec<Refusal>,
) {
    let wanted = request.intensity.get();
    let ceiling = profile.intensity_ceiling.get();
    if wanted > ceiling {
        out.push(Refusal::new(
            RefusalCode::Ceiling,
            format!("intensity {wanted} exceeds the profile ceiling {ceiling}"),
        ));
    }
    for class in &request.classes {
        let class_max = canon.class(*class).max_intensity;
        if wanted > class_max {
            out.push(Refusal::new(
                RefusalCode::Ceiling,
                format!("intensity {wanted} exceeds canon bound {class_max} for class {class}"),
            ));
        }
    }
    if request.duration_minutes > profile.max_session_minutes {
        out.push(Refusal::new(
            RefusalCode::Ceiling,
            format!(
                "duration {} min exceeds the profile maximum {} min",
                request.duration_minutes, profile.max_session_minutes
            ),
        ));
    }
}

fn check_programme(
    profile: &ConsentProfile,
    context: &SessionContext,
    request: &SessionRequest,
    out: &mut Vec<Refusal>,
) {
    if !request.programme_scoped || !profile.grants.longitudinal {
        // Without the grant, check_grants already refused.
        return;
    }
    // Invariant, not input: ConsentProfile::validate pairs the
    // longitudinal grant with its programme terms.
    let programme = profile
        .programme
        .expect("validated profile pairs the longitudinal grant with terms");
    if programme.is_expired(context.now_unix_s) {
        out.push(Refusal::new(
            RefusalCode::ProgrammeExpired,
            format!(
                "programme grant expired at {} (now {}); renew it in an alert state",
                programme.expires_at_unix_s, context.now_unix_s
            ),
        ));
    }
}

/// How an amendment relates to the profile it changes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "widens", rename_all = "snake_case")]
pub enum AmendKind {
    /// Within the current envelope (includes no change).
    Narrowing,
    /// Wider along the named dimensions; only outside a session.
    Widening(Vec<String>),
}

/// Gate a profile amendment. Narrowing is always allowed, from any
/// state; widening is refused while a session context is active.
pub fn decide_amend(
    current: &ConsentProfile,
    candidate: &ConsentProfile,
    context: &SessionContext,
) -> Result<AmendKind, Refusal> {
    assert_eq!(
        current.profile_id, candidate.profile_id,
        "amend keeps identity"
    );
    let widens = current.envelope().widenings(&candidate.envelope());
    if widens.is_empty() {
        return Ok(AmendKind::Narrowing);
    }
    if context.session_active {
        return Err(Refusal::new(
            RefusalCode::WidenInSession,
            format!(
                "consent cannot grow in trance: {}; amend again in an alert state",
                widens.join(", ")
            ),
        ));
    }
    Ok(AmendKind::Widening(widens))
}

/// Gate profile creation: creating consent is a widening from
/// nothing, so it is refused while a session context is active.
pub fn decide_create(context: &SessionContext) -> Result<(), Refusal> {
    if context.session_active {
        return Err(Refusal::new(
            RefusalCode::WidenInSession,
            "a consent profile is created in an alert state, never inside a session",
        ));
    }
    Ok(())
}
