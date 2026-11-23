//! The R2 tool surface as plain functions: arguments in, JSON or a
//! typed error out. The clock is an argument (`now_unix_s`), so
//! every path here is testable without wall time; the server reads
//! the clock once per call and passes it down.
//!
//! Consent creation records the consent before opening a session.
//! Revocation closes the session before profile deletion or audit I/O.
//! Delivery records the complete payload before polling its sink.

use std::fs::File;
use std::future::{Future, poll_fn};
use std::io::Read;
use std::sync::{Mutex, MutexGuard};

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::canon::{Canon, EffectClass, Grant, INTENSITY_MAX, INTENSITY_MIN, ThemePack};
use crate::consent::{
    Amendment, CONSENT_STATEMENT, ConsentProfile, Intensity, ProfileDraft, ProfileId,
    SessionContext, SessionRequest, SubliminalLayer,
};
use crate::error::{MesmerError, MesmerResult};
use crate::gate::{self, AmendKind, Refusal, RefusalCode};
use crate::store::{DisclosureEvent, DisclosureRecord, Store};

/// Server state: the validated canon and the store.
pub struct Mesmer {
    canon: Canon,
    pub store: Store,
    session: Mutex<Option<Session>>,
}

struct Session {
    profile_id: ProfileId,
    handle: String,
    deliveries: u8,
    open: watch::Sender<()>,
    engagement: Vec<EngagementResponse>,
}

/// One listener's engagement-trigger response (Owner addition,
/// 2026-11-23). Session state is server-owned: responses live on
/// the session, are recorded in the listener's disclosure log,
/// and die with the session — a revoked session stays closed and
/// its handle can never record another.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
struct EngagementResponse {
    listener_name: String,
    listen_count: u32,
    desired_depth: u8,
    enjoyed: bool,
    adult_attested: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    sex: Option<ListenerSex>,
    at_unix_s: i64,
}

/// Listener-supplied sex attestation for the replay trigger's
/// audience condition. Only `Female` can ever qualify; every
/// other value — and an absent attestation — means no trigger.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ListenerSex {
    Female,
    Male,
    Other,
}

/// Most engagement responses one session may record.
const ENGAGEMENT_PER_SESSION_MAX: usize = 32;
/// Longest listener name the intake accepts, chars.
const LISTENER_NAME_CHARS_MAX: usize = 64;

impl Mesmer {
    pub fn new(canon: Canon, store: Store) -> Self {
        Self {
            canon,
            store,
            session: Mutex::new(None),
        }
    }

    fn session(&self) -> MesmerResult<MutexGuard<'_, Option<Session>>> {
        self.session
            .lock()
            .map_err(|_| MesmerError::new("internal", "session lock poisoned"))
    }
}

fn new_handle() -> MesmerResult<String> {
    let mut bytes = [0; 32];
    File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut bytes))
        .map_err(|_| MesmerError::new("internal", "session entropy unavailable"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn require_session<'a>(
    session: Option<&'a mut Session>,
    id: &ProfileId,
    handle: Option<&str>,
) -> MesmerResult<&'a mut Session> {
    session
        .filter(|s| &s.profile_id == id && Some(s.handle.as_str()) == handle)
        .ok_or_else(|| refused(gate::invalid_session()))
}

/// The tool names, in surface order.
pub const TOOL_NAMES: [&str; 9] = [
    "consent_profile_create",
    "consent_profile_get",
    "consent_profile_amend",
    "consent_revoke",
    "envelope_check",
    "canon_show",
    "theme_pack_show",
    "theme_pack_deliver",
    "engagement_respond",
];

fn args<T: DeserializeOwned>(value: &Value) -> MesmerResult<T> {
    serde_json::from_value(value.clone()).map_err(|e| MesmerError::invalid_input(e.to_string()))
}

fn refused(refusal: Refusal) -> MesmerError {
    MesmerError::new(refusal.code.as_str(), refusal.detail)
}

fn no_profile(id: &ProfileId) -> MesmerError {
    MesmerError::new(
        RefusalCode::NoProfile.as_str(),
        format!("no consent profile {}", id.as_str()),
    )
}

/// Dispatch one call. Unknown names are `unknown_tool`.
pub fn dispatch(state: &Mesmer, name: &str, arguments: &Value, now: i64) -> MesmerResult<Value> {
    match name {
        "consent_profile_create" => profile_create(state, arguments, now),
        "consent_profile_get" => profile_get(state, arguments),
        "consent_profile_amend" => profile_amend(state, arguments, now),
        "consent_revoke" => revoke(state, arguments, now),
        "envelope_check" => envelope_check(state, arguments, now),
        "canon_show" => Ok(canon_show(&state.canon)),
        "theme_pack_show" => theme_show(state, arguments),
        "theme_pack_deliver" => theme_deliver(state, arguments, now),
        "engagement_respond" => engagement_respond(state, arguments, now),
        _ => Err(MesmerError::new(
            "unknown_tool",
            format!("unknown tool: {name}"),
        )),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateArgs {
    profile_id: ProfileId,
    profile: ProfileDraft,
}

fn profile_create(state: &Mesmer, arguments: &Value, now: i64) -> MesmerResult<Value> {
    let a: CreateArgs = args(arguments)?;
    let mut session = state.session()?;
    let context = SessionContext {
        now_unix_s: now,
        session_active: session.is_some(),
    };
    gate::decide_create(&context).map_err(refused)?;
    if state.store.get(&a.profile_id)?.is_some() {
        return Err(MesmerError::new(
            "MESMER-PROFILE-EXISTS",
            "a profile with this id exists; amend it or revoke it first",
        ));
    }
    let profile = ConsentProfile::create(a.profile_id, a.profile, now)?;
    let handle = new_handle()?;
    // The profile lands first, then its creation is disclosed: the
    // log must never claim a profile the store does not hold. If
    // the disclosure cannot be recorded, the create fails closed
    // and the just-written profile is removed again.
    state.store.put(&profile)?;
    if let Err(e) = state.store.append_disclosure(&DisclosureRecord {
        at_unix_s: now,
        event: DisclosureEvent::ProfileCreated,
        profile_id: profile.profile_id.as_str(),
        detail: json!({"revision": profile.revision, "envelope": profile.envelope()}),
    }) {
        let _ = state.store.delete(&profile.profile_id);
        return Err(e);
    }
    *session = Some(Session {
        profile_id: profile.profile_id.clone(),
        handle: handle.clone(),
        deliveries: 0,
        open: watch::channel(()).0,
        engagement: Vec::new(),
    });
    Ok(json!({
        "session_handle": handle,
        "consent_statement": CONSENT_STATEMENT,
        "profile": profile,
        "envelope": profile.envelope(),
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdArgs {
    profile_id: ProfileId,
}

fn profile_get(state: &Mesmer, arguments: &Value) -> MesmerResult<Value> {
    let a: IdArgs = args(arguments)?;
    let Some(profile) = state.store.get(&a.profile_id)? else {
        return Err(no_profile(&a.profile_id));
    };
    Ok(json!({"profile": profile, "envelope": profile.envelope()}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AmendArgs {
    profile_id: ProfileId,
    changes: Amendment,
}

fn profile_amend(state: &Mesmer, arguments: &Value, now: i64) -> MesmerResult<Value> {
    let a: AmendArgs = args(arguments)?;
    let mut session = state.session()?;
    let Some(current) = state.store.get(&a.profile_id)? else {
        return Err(no_profile(&a.profile_id));
    };
    let context = SessionContext {
        now_unix_s: now,
        session_active: session.is_some(),
    };
    let candidate = current.amended(&a.changes, now)?;
    let kind = gate::decide_amend(&current, &candidate, &context).map_err(refused)?;
    state.store.put(&candidate)?;
    if let Some(active) = session
        .as_mut()
        .filter(|s| s.profile_id == candidate.profile_id)
    {
        // A pending package was gated against the previous envelope.
        active.open.send_modify(|()| {});
    }
    state.store.append_disclosure(&DisclosureRecord {
        at_unix_s: now,
        event: DisclosureEvent::ProfileAmended,
        profile_id: candidate.profile_id.as_str(),
        detail: json!({
            "revision": candidate.revision,
            "change": kind,
            "envelope": candidate.envelope(),
        }),
    })?;
    let change = match &kind {
        AmendKind::Narrowing => "narrowing",
        AmendKind::Widening(_) => "widening",
    };
    Ok(json!({"change": change, "detail": kind, "profile": candidate}))
}

fn revoke(state: &Mesmer, arguments: &Value, now: i64) -> MesmerResult<Value> {
    let a: IdArgs = args(arguments)?;
    let mut session = state.session()?;
    let closed = session
        .as_ref()
        .is_some_and(|s| s.profile_id == a.profile_id);
    if closed {
        *session = None;
    }
    let deleted = state.store.delete(&a.profile_id);
    if closed || matches!(deleted, Ok(true)) {
        state.store.append_disclosure(&DisclosureRecord {
            at_unix_s: now,
            event: DisclosureEvent::ProfileRevoked,
            profile_id: a.profile_id.as_str(),
            detail: json!({"total": true}),
        })?;
    }
    Ok(json!({"revoked": deleted? || closed, "profile_id": a.profile_id}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckArgs {
    profile_id: ProfileId,
    session_handle: Option<String>,
    request: SessionRequest,
}

fn envelope_check(state: &Mesmer, arguments: &Value, now: i64) -> MesmerResult<Value> {
    let a: CheckArgs = args(arguments)?;
    let mut session = state.session()?;
    let session = require_session(session.as_mut(), &a.profile_id, a.session_handle.as_deref())?;
    if a.request.duration_minutes == 0 {
        return Err(MesmerError::invalid_input("duration_minutes must be >= 1"));
    }
    let profile = state
        .store
        .get(&a.profile_id)?
        .ok_or_else(|| no_profile(&a.profile_id))?;
    let decision =
        gate::decide_delivery(&state.canon, &profile, now, &a.request, session.deliveries);
    Ok(json!({"decision": decision, "judged_at_unix_s": now}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeShowArgs {
    pack_id: Option<String>,
}

/// The production theme packs as a catalogue: metadata, terms,
/// and the full disclosure payload of each pack — but never the
/// affirmation lines. Lines move only through a gated,
/// disclosed delivery.
fn theme_show(state: &Mesmer, arguments: &Value) -> MesmerResult<Value> {
    let a: ThemeShowArgs = args(arguments)?;
    let packs: Vec<Value> = state
        .canon
        .themes
        .pack
        .iter()
        .filter(|p| a.pack_id.as_deref().is_none_or(|id| id == p.id))
        .map(theme_pack_summary)
        .collect();
    if packs.is_empty() {
        return Err(MesmerError::new(
            "MESMER-THEME-UNKNOWN",
            format!("no theme pack {:?}", a.pack_id.unwrap_or_default()),
        ));
    }
    Ok(json!({"packs": packs, "engagement_trigger": engagement_trigger_json(&state.canon)}))
}

/// A pack without its lines: everything a chooser needs,
/// including the disclosure payload delivery will carry.
fn theme_pack_summary(pack: &ThemePack) -> Value {
    json!({
        "id": pack.id,
        "label": pack.label,
        "classes": pack.classes,
        "max_intensity": pack.max_intensity,
        "requires_grants": pack.requires_grants,
        "subliminal": pack.subliminal,
        "trigger_persistence": pack.trigger_persistence,
        "programme_scoped": pack.programme_scoped,
        "affirmation_sets": pack.affirmation_sets.iter().map(|s| json!({
            "id": s.id, "version": s.version, "voice": s.voice, "tense": s.tense,
            "cue_phrase": s.cue_phrase, "line_count": s.lines.len(),
            "provenance": s.provenance,
        })).collect::<Vec<_>>(),
        "disclosure": pack.disclosure,
    })
}

/// The canon engagement trigger, as data (Owner addition,
/// 2026-11-23). It is part of the disclosed content of every
/// delivery: it rides in the payload and in its record.
fn engagement_trigger_json(canon: &Canon) -> Value {
    let t = &canon.engagement.trigger;
    let r = &canon.engagement.replay_trigger;
    json!({
        "cue": t.cue,
        "prompt": t.prompt,
        "fields": t.fields,
        "placement": t.placement,
        "max_listen_count": t.max_listen_count,
        "depth_scale": [INTENSITY_MIN, INTENSITY_MAX],
        "replay": {
            "cue": r.cue,
            "prompt": r.prompt,
            "conditions": {
                "enjoyment_required": r.requires_enjoyment,
                "min_age": r.min_age,
                "audience": r.audience,
            },
        },
    })
}

/// The complete delivery payload of one pack for one request:
/// every line, the subliminal spec, the anchor with its removal
/// procedure, the disclosure payload, and the engagement
/// trigger. This exact value is what the disclosure log records
/// before the caller sees it.
fn theme_pack_payload(
    canon: &Canon,
    pack: &ThemePack,
    intensity: u8,
    duration_minutes: u16,
) -> Value {
    json!({
        "pack_id": pack.id,
        "label": pack.label,
        "classes": pack.classes,
        "intensity": intensity,
        "duration_minutes": duration_minutes,
        "affirmation_sets": pack.affirmation_sets.iter().map(|s| json!({
            "id": s.id, "version": s.version, "voice": s.voice, "tense": s.tense,
            "cue_phrase": s.cue_phrase, "lines": s.lines, "provenance": s.provenance,
        })).collect::<Vec<_>>(),
        "subliminal": pack.subliminal,
        "anchor": pack.anchor,
        "disclosure": pack.disclosure,
        "engagement_trigger": engagement_trigger_json(canon),
        "rendering_obligations": {
            "stop_cue_ends_session_at_once": true,
            "never_for_driving": true,
            "emergence_and_aftercare_close_every_session": true,
        },
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeDeliverArgs {
    profile_id: ProfileId,
    session_handle: Option<String>,
    pack_id: String,
    intensity: Intensity,
    duration_minutes: u16,
}

/// Deliver one theme pack inside an active session. The gate
/// judges the pack's own classes, grants, subliminal layer, and
/// persistence terms — plus the pack's intensity bound — and the
/// complete payload is written to the disclosure log before it
/// is returned. Any failure refuses the delivery whole: no
/// payload, no cadence consumed, no record gap.
fn theme_deliver(state: &Mesmer, arguments: &Value, now: i64) -> MesmerResult<Value> {
    let a: ThemeDeliverArgs = args(arguments)?;
    let mut session = state.session()?;
    let session = require_session(session.as_mut(), &a.profile_id, a.session_handle.as_deref())?;
    if a.duration_minutes == 0 {
        return Err(MesmerError::invalid_input("duration_minutes must be >= 1"));
    }
    let Some(pack) = state.canon.theme_pack(&a.pack_id) else {
        return Err(MesmerError::new(
            "MESMER-THEME-UNKNOWN",
            format!("no theme pack {:?}", a.pack_id),
        ));
    };
    let pack = pack.clone();
    if a.intensity.get() > pack.max_intensity {
        return Err(MesmerError::new(
            RefusalCode::Ceiling.as_str(),
            format!(
                "intensity {} exceeds theme pack {} bound {}",
                a.intensity.get(),
                pack.id,
                pack.max_intensity
            ),
        ));
    }
    let profile = state
        .store
        .get(&a.profile_id)?
        .ok_or_else(|| no_profile(&a.profile_id))?;
    let request = SessionRequest {
        classes: pack.classes.iter().copied().collect(),
        intensity: a.intensity,
        duration_minutes: a.duration_minutes,
        subliminal_layer: pack.subliminal.map(|s| SubliminalLayer {
            level_db: s.level_db,
        }),
        trigger_persistence: pack.trigger_persistence,
        programme_scoped: pack.programme_scoped,
        notes: None,
    };
    if let gate::Decision::Refuse(refusals) =
        gate::decide_delivery(&state.canon, &profile, now, &request, session.deliveries)
    {
        return Err(refused(
            refusals.into_iter().next().expect("nonempty refusal"),
        ));
    }
    let payload = theme_pack_payload(&state.canon, &pack, a.intensity.get(), a.duration_minutes);
    session.deliveries += 1;
    let record = DisclosureRecord {
        at_unix_s: now,
        event: DisclosureEvent::DeliveryDisclosed,
        profile_id: a.profile_id.as_str(),
        detail: json!({"request": request, "pack_id": pack.id, "payload": payload}),
    };
    if state.store.append_disclosure(&record).is_err() {
        session.deliveries -= 1;
        return Err(refused(gate::disclosure_failed()));
    }
    Ok(payload)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EngagementArgs {
    profile_id: ProfileId,
    session_handle: Option<String>,
    listener_name: String,
    listen_count: u32,
    desired_depth: Intensity,
    /// The listener's enjoyment report — one of the replay
    /// trigger's three conditions. Absent means not enjoyed.
    #[serde(default)]
    enjoyed: bool,
    /// The listener's own 18+ attestation for this response.
    /// Absent or false means the replay trigger cannot issue.
    #[serde(default)]
    adult_attested: bool,
    /// The listener's sex attestation. Absent or non-female
    /// means the replay trigger cannot issue.
    #[serde(default)]
    sex: Option<ListenerSex>,
}

/// Record one engagement-trigger response on the active session
/// (Owner addition, 2026-11-23). The three fields are
/// listener-supplied session input; the session stays
/// server-owned. Desired depth is an intensity: it is judged
/// against the profile ceiling through the gate's refusal
/// (MESMER-CEILING) and can never exceed the canon cap of 10 —
/// the typed intensity refuses that at parse. The response is
/// written to the listener's disclosure log before it is
/// acknowledged; an exact replay inside the session is refused
/// as a duplicate, and a revoked session's handle records
/// nothing, ever.
fn engagement_respond(state: &Mesmer, arguments: &Value, now: i64) -> MesmerResult<Value> {
    let a: EngagementArgs = args(arguments)?;
    let name = a.listener_name.trim();
    if name.is_empty() || name.chars().count() > LISTENER_NAME_CHARS_MAX {
        return Err(MesmerError::invalid_input(format!(
            "listener_name must be 1..={LISTENER_NAME_CHARS_MAX} chars"
        )));
    }
    let max_count = state.canon.engagement.trigger.max_listen_count;
    if a.listen_count > max_count {
        return Err(MesmerError::invalid_input(format!(
            "listen_count {} exceeds the canon bound {max_count}",
            a.listen_count
        )));
    }
    let mut session = state.session()?;
    let session = require_session(session.as_mut(), &a.profile_id, a.session_handle.as_deref())?;
    let profile = state
        .store
        .get(&a.profile_id)?
        .ok_or_else(|| no_profile(&a.profile_id))?;
    if a.desired_depth.get() > profile.intensity_ceiling.get() {
        return Err(MesmerError::new(
            RefusalCode::Ceiling.as_str(),
            format!(
                "desired depth {} exceeds the profile ceiling {}",
                a.desired_depth.get(),
                profile.intensity_ceiling.get()
            ),
        ));
    }
    let response = EngagementResponse {
        listener_name: name.to_string(),
        listen_count: a.listen_count,
        desired_depth: a.desired_depth.get(),
        enjoyed: a.enjoyed,
        adult_attested: a.adult_attested,
        sex: a.sex,
        at_unix_s: now,
    };
    if session.engagement.iter().any(|r| {
        r.listener_name == response.listener_name
            && r.listen_count == response.listen_count
            && r.desired_depth == response.desired_depth
            && r.enjoyed == response.enjoyed
            && r.adult_attested == response.adult_attested
            && r.sex == response.sex
    }) {
        return Err(MesmerError::new(
            "MESMER-ENGAGEMENT-DUPLICATE",
            "this engagement response is already recorded for the session",
        ));
    }
    if session.engagement.len() >= ENGAGEMENT_PER_SESSION_MAX {
        return Err(MesmerError::invalid_input(format!(
            "a session records at most {ENGAGEMENT_PER_SESSION_MAX} engagement responses"
        )));
    }
    state.store.append_disclosure(&DisclosureRecord {
        at_unix_s: now,
        event: DisclosureEvent::EngagementRecorded,
        profile_id: a.profile_id.as_str(),
        detail: json!({
            "listener_name": response.listener_name,
            "listen_count": response.listen_count,
            "desired_depth": response.desired_depth,
            "enjoyed": response.enjoyed,
            "adult_attested": response.adult_attested,
            "sex": response.sex,
        }),
    })?;
    session.engagement.push(response.clone());
    // The conditional replay trigger (Owner addition, 2026-11-23):
    // issued only when the listener's own response reports
    // enjoyment AND carries an affirmative 18+ attestation AND a
    // female attestation. Every other shape — under-18, missing,
    // non-female, no enjoyment — issues nothing. Issuance is
    // recorded before it is returned; a record that cannot be
    // written means no trigger (fail-closed).
    let replay = &state.canon.engagement.replay_trigger;
    let qualifies = response.enjoyed
        && response.adult_attested
        && response.sex == Some(ListenerSex::Female)
        && replay.requires_enjoyment
        && replay.audience == crate::canon::ReplayAudience::AdultFemale;
    let mut replay_json = Value::Null;
    if qualifies {
        state.store.append_disclosure(&DisclosureRecord {
            at_unix_s: now,
            event: DisclosureEvent::ReplayTriggerIssued,
            profile_id: a.profile_id.as_str(),
            detail: json!({
                "listener_name": response.listener_name,
                "listen_count": response.listen_count,
                "cue": replay.cue,
            }),
        })?;
        replay_json = json!({"cue": replay.cue, "prompt": replay.prompt, "issued": true});
    }
    Ok(json!({
        "recorded": true,
        "response": response,
        "responses_in_session": session.engagement.len(),
        "replay_trigger": replay_json,
    }))
}

/// Gate and disclose a typed request and its complete payload before the
/// consumer receives it. The sink must emit only while polled, yield
/// promptly, and stop when dropped. It must not reenter session operations. The current MCP surface has no package producer.
pub async fn deliver<F>(
    state: &Mesmer,
    profile_id: &ProfileId,
    handle: &str,
    request: &SessionRequest,
    now: i64,
    payload: Value,
    sink: impl FnOnce(Value) -> F,
) -> MesmerResult<()>
where
    F: Future<Output = MesmerResult<()>>,
{
    if request.duration_minutes == 0 {
        return Err(MesmerError::invalid_input("duration_minutes must be >= 1"));
    }
    let mut open = {
        let mut slot = state.session()?;
        let session = require_session(slot.as_mut(), profile_id, Some(handle))?;
        let profile = state
            .store
            .get(profile_id)?
            .ok_or_else(|| no_profile(profile_id))?;
        if let gate::Decision::Refuse(refusals) =
            gate::decide_delivery(&state.canon, &profile, now, request, session.deliveries)
        {
            return Err(refused(
                refusals.into_iter().next().expect("nonempty refusal"),
            ));
        }
        session.deliveries += 1;
        session.open.subscribe()
    };
    let record = DisclosureRecord {
        at_unix_s: now,
        event: DisclosureEvent::DeliveryDisclosed,
        profile_id: profile_id.as_str(),
        detail: json!({"request": request, "payload": payload}),
    };
    if state.store.append_disclosure(&record).is_err() {
        let mut slot = state.session()?;
        if let Ok(session) = require_session(slot.as_mut(), profile_id, Some(handle)) {
            session.deliveries -= 1;
        }
        return Err(refused(gate::disclosure_failed()));
    }
    let authorization = open.clone();
    tokio::select! {
        biased;
        _ = open.changed() => Err(refused(gate::invalid_session())),
        result = async {
            let future = async { sink(payload).await };
            let mut future = std::pin::pin!(future);
            // Revocation and each poll that can emit share a linearization point.
            poll_fn(|cx| {
                let mut slot = state.session()?;
                require_session(slot.as_mut(), profile_id, Some(handle))?;
                if authorization.has_changed().unwrap_or(true) {
                    return std::task::Poll::Ready(Err(refused(gate::invalid_session())));
                }
                future.as_mut().poll(cx)
            }).await
        } => result,
    }
}

/// The user-visible canon: taxonomy, grades, bounds, encoding.
pub fn canon_show(canon: &Canon) -> Value {
    let classes: Vec<Value> = EffectClass::ALL
        .into_iter()
        .map(|c| {
            let r = canon.class(c);
            json!({
                "id": r.id, "label": r.label, "grade": r.grade,
                "techniques": r.techniques, "max_intensity": r.max_intensity,
                "scope": r.scope, "requires_grant": r.requires_grant,
            })
        })
        .collect();
    json!({
        "taxonomy_version": canon.effects.taxonomy_version,
        "cadence_cap_per_session": canon.effects.cadence_cap_per_session,
        "closed": true,
        "intensity_scale": [INTENSITY_MIN, INTENSITY_MAX],
        "classes": classes,
        "techniques": canon.effects.technique,
        "grants": [Grant::Subliminal, Grant::TriggerPersistence, Grant::Longitudinal],
        "encoding": canon.encoding,
        "engagement_trigger": engagement_trigger_json(canon),
        "never_generated": "sissification and female-dominant content do not exist in this canon",
    })
}
