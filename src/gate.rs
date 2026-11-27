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
use crate::consent::{
    CompositeSpec, ConsentProfile, EndStatePersistence, PlaylistDefinition, SessionContext,
    SessionRequest,
};

/// The refusal taxonomy (fleet convention: stable `MESMER-*` codes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum RefusalCode {
    #[serde(rename = "MESMER-SESSION-INVALID")]
    SessionInvalid,
    #[serde(rename = "MESMER-CADENCE-CAP")]
    CadenceCap,
    #[serde(rename = "MESMER-DISCLOSURE-FAILED")]
    DisclosureFailed,
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
    #[serde(rename = "MESMER-PLAYLIST-INVALID")]
    PlaylistInvalid,
    #[serde(rename = "MESMER-PLAYLIST-SCOPE")]
    PlaylistScope,
    #[serde(rename = "MESMER-ENDSTATE")]
    EndState,
    #[serde(rename = "MESMER-PLAYLIST-STATE")]
    PlaylistState,
    #[serde(rename = "MESMER-PLAYLIST-UNKNOWN")]
    PlaylistUnknown,
}

impl RefusalCode {
    pub fn as_str(self) -> &'static str {
        match self {
            RefusalCode::DisclosureFailed => "MESMER-DISCLOSURE-FAILED",
            RefusalCode::CadenceCap => "MESMER-CADENCE-CAP",
            RefusalCode::SessionInvalid => "MESMER-SESSION-INVALID",
            RefusalCode::NoProfile => "MESMER-NO-PROFILE",
            RefusalCode::EnvelopeClass => "MESMER-ENVELOPE-CLASS",
            RefusalCode::GrantAbsent => "MESMER-GRANT-ABSENT",
            RefusalCode::Ceiling => "MESMER-CEILING",
            RefusalCode::ProgrammeExpired => "MESMER-PROGRAMME-EXPIRED",
            RefusalCode::WidenInSession => "MESMER-WIDEN-IN-SESSION",
            RefusalCode::PlaylistInvalid => "MESMER-PLAYLIST-INVALID",
            RefusalCode::PlaylistScope => "MESMER-PLAYLIST-SCOPE",
            RefusalCode::EndState => "MESMER-ENDSTATE",
            RefusalCode::PlaylistState => "MESMER-PLAYLIST-STATE",
            RefusalCode::PlaylistUnknown => "MESMER-PLAYLIST-UNKNOWN",
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

pub(crate) fn invalid_session() -> Refusal {
    Refusal::new(
        RefusalCode::SessionInvalid,
        "an active session handle is required",
    )
}

pub(crate) fn disclosure_failed() -> Refusal {
    Refusal::new(
        RefusalCode::DisclosureFailed,
        "delivery disclosure could not be recorded",
    )
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

/// Apply the session's server-owned delivery count after the envelope checks.
pub(crate) fn decide_delivery(
    canon: &Canon,
    profile: &ConsentProfile,
    now: i64,
    request: &SessionRequest,
    deliveries: u8,
) -> Decision {
    let context = SessionContext {
        now_unix_s: now,
        session_active: true,
    };
    let mut decision = decide(canon, Some(profile), &context, request);
    if deliveries >= canon.effects.cadence_cap_per_session {
        let refusal = Refusal::new(
            RefusalCode::CadenceCap,
            "session delivery cadence cap reached",
        );
        match &mut decision {
            Decision::Allow => decision = Decision::Refuse(vec![refusal]),
            Decision::Refuse(refusals) => refusals.push(refusal),
        }
    }
    decision
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

// ── Playlists and composites (1.1.0, playlists SPEC §4.3) ───────
//
// `decide` and `decide_delivery` above are unchanged: the
// playlist feature calls them, it does not amend them. The
// functions below are new pure judgements beside them.

/// Gate playlist authoring (SPEC §2.1): a chain is a
/// consent-adjacent artefact and is never authored from inside a
/// session. The caller scopes `context.session_active` to
/// sessions bound to the playlist's own profile (SPEC §0,
/// concurrent-world restatement).
pub fn decide_playlist_create(
    _profile: &ConsentProfile,
    context: &SessionContext,
) -> Result<(), Refusal> {
    if context.session_active {
        return Err(Refusal::new(
            RefusalCode::WidenInSession,
            "a playlist is authored in an alert state, never inside a session on its profile",
        ));
    }
    Ok(())
}

/// Judge every entry of a playlist against the profile as it
/// stands, plus the end state's terms (SPEC §2.4): each entry is
/// judged by the existing [`decide`], refusals in entry order,
/// then the end state is checked against the envelope and the
/// ceiling. Shared by authoring-time validation (§2.1) and the
/// run-start check (§4.3).
pub fn judge_playlist(
    canon: &Canon,
    profile: &ConsentProfile,
    context: &SessionContext,
    playlist: &PlaylistDefinition,
) -> Vec<Refusal> {
    let mut refusals = Vec::new();
    for (i, _) in playlist.entries.iter().enumerate() {
        let request = playlist.entry_request(canon, i);
        if let Decision::Refuse(rs) = decide(canon, Some(profile), context, &request) {
            refusals.extend(rs.into_iter().map(|r| Refusal {
                code: r.code,
                detail: format!("entry {i}: {}", r.detail),
            }));
        }
    }
    for class in &playlist.end_state.classes {
        if !profile.allowed_classes.contains(class) {
            refusals.push(Refusal::new(
                RefusalCode::EndState,
                format!("end state class {class} is outside the envelope"),
            ));
        }
    }
    let depth = playlist.end_state.depth_target.get();
    let ceiling = profile.intensity_ceiling.get();
    if depth > ceiling {
        refusals.push(Refusal::new(
            RefusalCode::Ceiling,
            format!("end state depth {depth} exceeds the profile ceiling {ceiling}"),
        ));
    }
    refusals
}

/// The programme day-cadence refusal, shared by the start gate
/// and the run-advance check in the tool layer.
pub fn programme_day_cap_refusal(cap: u8, deliveries_today: u32) -> Refusal {
    Refusal::new(
        RefusalCode::CadenceCap,
        format!(
            "programme day cadence cap {cap} reached for today ({deliveries_today} deliveries disclosed)"
        ),
    )
}

/// Gate a run's start (SPEC §4.3): the §2.4 start-level check —
/// [`judge_playlist`] against the profile as it now stands —
/// plus the programme day-cadence check for a programme-scoped
/// playlist. `deliveries_today` is the profile's disclosed
/// delivery count for the current day, read by the caller from
/// the disclosure log (the gate reads no store).
pub fn decide_playlist_start(
    canon: &Canon,
    profile: &ConsentProfile,
    context: &SessionContext,
    playlist: &PlaylistDefinition,
    deliveries_today: u32,
) -> Decision {
    let mut refusals = judge_playlist(canon, profile, context, playlist);
    if playlist.end_state.persistence == EndStatePersistence::Programme
        && let Some(programme) = profile.programme
        && deliveries_today >= u32::from(programme.cadence_cap_per_day)
    {
        refusals.push(programme_day_cap_refusal(
            programme.cadence_cap_per_day,
            deliveries_today,
        ));
    }
    if refusals.is_empty() {
        Decision::Allow
    } else {
        Decision::Refuse(refusals)
    }
}

/// Judge a composite delivery (SPEC §3.3): two passes, then one
/// verdict. Per layer, the pack is judged exactly as its
/// standalone delivery would be; then the union request is
/// judged **once** by the existing [`decide_delivery`], cadence
/// included. On success the union request is returned for
/// disclosure; on failure every refusal, layers first.
pub fn decide_composite(
    canon: &Canon,
    profile: &ConsentProfile,
    context: &SessionContext,
    composite: &CompositeSpec,
    deliveries: u8,
) -> Result<SessionRequest, Vec<Refusal>> {
    let mut refusals = Vec::new();
    for layer in &composite.layers {
        let Some(pack) = canon.theme_pack(&layer.pack_id) else {
            return Err(vec![Refusal::new(
                RefusalCode::PlaylistInvalid,
                format!("no theme pack {:?}", layer.pack_id),
            )]);
        };
        if composite.intensity.get() > pack.max_intensity {
            refusals.push(Refusal::new(
                RefusalCode::Ceiling,
                format!(
                    "layer {} ({}): intensity {} exceeds theme pack {} bound {}",
                    layer.role,
                    pack.id,
                    composite.intensity.get(),
                    pack.id,
                    pack.max_intensity
                ),
            ));
        }
        let request =
            crate::consent::pack_request(pack, composite.intensity, composite.duration_minutes);
        if let Decision::Refuse(rs) = decide(canon, Some(profile), context, &request) {
            refusals.extend(rs.into_iter().map(|r| Refusal {
                code: r.code,
                detail: format!("layer {} ({}): {}", layer.role, pack.id, r.detail),
            }));
        }
    }
    let union = composite.request(canon);
    if union.trigger_persistence
        && union
            .classes
            .iter()
            .any(|c| canon.class(*c).scope == Scope::SessionOnly)
    {
        refusals.push(Refusal::new(
            RefusalCode::PlaylistScope,
            "a composite may not claim persistence for a session-only class",
        ));
    }
    if let Decision::Refuse(rs) =
        decide_delivery(canon, profile, context.now_unix_s, &union, deliveries)
    {
        refusals.extend(rs);
    }
    if refusals.is_empty() {
        Ok(union)
    } else {
        Err(refusals)
    }
}
