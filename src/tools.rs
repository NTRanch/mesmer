//! The R2 tool surface as plain functions: arguments in, JSON or a
//! typed error out. The clock is an argument (`now_unix_s`), so
//! every path here is testable without wall time; the server reads
//! the clock once per call and passes it down.
//!
//! Consent creation records the consent before opening a session.
//! Revocation closes the session before profile deletion or audit I/O.
//! Delivery records the complete payload before polling its sink.

use std::collections::HashMap;
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
    Amendment, CONSENT_STATEMENT, CompositeSpec, ConsentProfile, EndStatePersistence, Intensity,
    PlaylistDefinition, PlaylistDraft, PlaylistEntry, PlaylistId, ProfileDraft, ProfileId,
    SessionContext, SessionRequest, SubliminalLayer,
};
use crate::error::{MesmerError, MesmerResult};
use crate::gate::{self, AmendKind, Refusal, RefusalCode};
use crate::store::{DisclosureEvent, DisclosureRecord, Store};

/// Server state: the validated canon and the store.
pub struct Mesmer {
    canon: Canon,
    pub store: Store,
    sessions: Mutex<HashMap<String, Session>>,
    runs: Mutex<HashMap<String, Run>>,
}

struct Session {
    profile_id: ProfileId,
    handle: String,
    deliveries: u8,
    open: watch::Sender<()>,
    engagement: Vec<EngagementResponse>,
    /// The run this session is an entry of, if any (playlists
    /// SPEC §4.5): the session's position is the entry index.
    run: Option<RunRef>,
}

/// A session's binding to its playlist run.
#[derive(Clone, Debug)]
struct RunRef {
    playlist_id: PlaylistId,
    position: usize,
}

/// Server-side run state (playlists SPEC §1.3), held beside the
/// session map and never persisted: a server restart closes
/// every session and run, leaving disclosed records and no run.
/// A run exists only while one of its entry sessions is open —
/// advancing is atomic inside `session_close` — so its phase is
/// always `in_session`; the spec's between-sessions moment is
/// the close call itself, not a resting state.
struct Run {
    profile_id: ProfileId,
    playlist_id: PlaylistId,
    /// The entry whose session is currently open.
    position: usize,
    phase: RunPhase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum RunPhase {
    InSession,
}

/// The runs map's key: one active run per playlist (SPEC §1.3).
fn run_key(profile: &ProfileId, playlist: &PlaylistId) -> String {
    format!("{}/{}", profile.as_str(), playlist.as_str())
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
            sessions: Mutex::new(HashMap::new()),
            runs: Mutex::new(HashMap::new()),
        }
    }

    fn sessions(&self) -> MesmerResult<MutexGuard<'_, HashMap<String, Session>>> {
        self.sessions
            .lock()
            .map_err(|_| MesmerError::new("internal", "session lock poisoned"))
    }

    fn runs(&self) -> MesmerResult<MutexGuard<'_, HashMap<String, Run>>> {
        self.runs
            .lock()
            .map_err(|_| MesmerError::new("internal", "run lock poisoned"))
    }
}

fn new_handle() -> MesmerResult<String> {
    let mut bytes = [0; 32];
    File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut bytes))
        .map_err(|_| MesmerError::new("internal", "session entropy unavailable"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Address one session in the map by its handle (playlists SPEC
/// §0, concurrent model): a handle addresses exactly one
/// session, and presenting session A's handle against profile B
/// refuses identically to a forged, missing, or replayed handle.
fn require_session<'a>(
    sessions: &'a mut HashMap<String, Session>,
    id: &ProfileId,
    handle: Option<&str>,
) -> MesmerResult<&'a mut Session> {
    let handle = handle.ok_or_else(|| refused(gate::invalid_session()))?;
    sessions
        .get_mut(handle)
        .filter(|s| &s.profile_id == id)
        .ok_or_else(|| refused(gate::invalid_session()))
}

/// Whether any session in the map is bound to `id`.
fn any_session_for(sessions: &HashMap<String, Session>, id: &ProfileId) -> bool {
    sessions.values().any(|s| &s.profile_id == id)
}

/// Open a session on a profile and record it. The opening is
/// disclosed before the handle is returned; if the disclosure
/// cannot be recorded, no session opens (fail-closed, as every
/// mesmer act). A run entry's opening additionally records the
/// entry and the request it was judged by. The session opened
/// by profile creation passes `record_opening = false`: its
/// opening is already recorded by `ProfileCreated`, and the
/// 1.0.0 disclosure shape of a create does not change.
fn open_session(
    state: &Mesmer,
    sessions: &mut HashMap<String, Session>,
    profile_id: &ProfileId,
    run: Option<RunRef>,
    now: i64,
    entry_request: Option<&SessionRequest>,
    record_opening: bool,
) -> MesmerResult<String> {
    let handle = new_handle()?;
    sessions.insert(
        handle.clone(),
        Session {
            profile_id: profile_id.clone(),
            handle: handle.clone(),
            deliveries: 0,
            open: watch::channel(()).0,
            engagement: Vec::new(),
            run: run.clone(),
        },
    );
    let disclosed = if record_opening {
        state.store.append_disclosure(&DisclosureRecord {
            at_unix_s: now,
            event: DisclosureEvent::SessionOpened,
            profile_id: profile_id.as_str(),
            detail: json!({
                "handle_prefix": &handle[..8],
                "run": run.as_ref().map(|r| json!({
                    "playlist_id": r.playlist_id, "position": r.position,
                })),
            }),
        })
    } else {
        Ok(())
    };
    let entry_disclosed = match (&run, entry_request) {
        (Some(r), Some(request)) => state.store.append_disclosure(&DisclosureRecord {
            at_unix_s: now,
            event: DisclosureEvent::PlaylistEntryOpened,
            profile_id: profile_id.as_str(),
            detail: json!({
                "playlist_id": r.playlist_id,
                "entry": r.position,
                "handle_prefix": &handle[..8],
                "request": request,
            }),
        }),
        _ => Ok(()),
    };
    if disclosed.is_err() || entry_disclosed.is_err() {
        sessions.remove(&handle);
        return Err(refused(gate::disclosure_failed()));
    }
    Ok(handle)
}

/// Record a session's closure.
fn disclose_closed(state: &Mesmer, session: &Session, now: i64) -> MesmerResult<()> {
    state.store.append_disclosure(&DisclosureRecord {
        at_unix_s: now,
        event: DisclosureEvent::SessionClosed,
        profile_id: session.profile_id.as_str(),
        detail: json!({
            "handle_prefix": &session.handle[..8],
            "deliveries": session.deliveries,
            "run": session.run.as_ref().map(|r| json!({
                "playlist_id": r.playlist_id, "position": r.position,
            })),
        }),
    })
}

/// Record a run's halt (playlists SPEC §4.5): the reason, the
/// refusal code when the halt is a refusal, and the position the
/// run halted at.
fn disclose_run_halted(
    state: &Mesmer,
    profile_id: &ProfileId,
    playlist_id: &PlaylistId,
    position: usize,
    reason: &str,
    refusal: Option<&Refusal>,
    now: i64,
) -> MesmerResult<()> {
    state.store.append_disclosure(&DisclosureRecord {
        at_unix_s: now,
        event: DisclosureEvent::PlaylistRunHalted,
        profile_id: profile_id.as_str(),
        detail: json!({
            "playlist_id": playlist_id,
            "position": position,
            "reason": reason,
            "refusal": refusal.map(|r| json!({"code": r.code, "detail": r.detail})),
        }),
    })
}

/// The tool names, in surface order.
pub const TOOL_NAMES: [&str; 15] = [
    "consent_profile_create",
    "consent_profile_get",
    "consent_profile_amend",
    "consent_revoke",
    "envelope_check",
    "canon_show",
    "theme_pack_show",
    "theme_pack_deliver",
    "engagement_respond",
    "session_open",
    "session_close",
    "playlist_create",
    "playlist_show",
    "playlist_start",
    "composite_deliver",
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
        "session_open" => session_open(state, arguments, now),
        "session_close" => session_close(state, arguments, now),
        "playlist_create" => playlist_create(state, arguments, now),
        "playlist_show" => playlist_show(state, arguments),
        "playlist_start" => playlist_start(state, arguments, now),
        "composite_deliver" => composite_deliver(state, arguments, now),
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
    let mut sessions = state.sessions()?;
    // Creating consent is refused while any session is active:
    // the 1.0.0 global alert-state rule, unchanged by the
    // concurrent model (the per-profile restatement in SPEC §0
    // covers widening and playlist authoring, not creation).
    let context = SessionContext {
        now_unix_s: now,
        session_active: !sessions.is_empty(),
    };
    gate::decide_create(&context).map_err(refused)?;
    if state.store.get(&a.profile_id)?.is_some() {
        return Err(MesmerError::new(
            "MESMER-PROFILE-EXISTS",
            "a profile with this id exists; amend it or revoke it first",
        ));
    }
    let profile = ConsentProfile::create(a.profile_id, a.profile, now)?;
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
    let handle = open_session(
        state,
        &mut sessions,
        &profile.profile_id,
        None,
        now,
        None,
        false,
    )?;
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
    let mut sessions = state.sessions()?;
    let Some(current) = state.store.get(&a.profile_id)? else {
        return Err(no_profile(&a.profile_id));
    };
    // SPEC §0 (concurrent-world restatement, spec position
    // implemented as written): the widening refusal is scoped
    // per profile — a session bound to another profile does not
    // freeze this profile's alert-state acts. This is the one
    // place the scoping lives; a ruling change edits it here.
    let context = SessionContext {
        now_unix_s: now,
        session_active: any_session_for(&sessions, &a.profile_id),
    };
    let candidate = current.amended(&a.changes, now)?;
    let kind = gate::decide_amend(&current, &candidate, &context).map_err(refused)?;
    state.store.put(&candidate)?;
    for active in sessions
        .values_mut()
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
    let mut sessions = state.sessions()?;
    let mut runs = state.runs()?;
    // Total, and scoped to the revoked profile (SPEC §1.4): all
    // of its sessions close, any run bound to it dies with it,
    // and its playlists are deleted in the same act. Sessions
    // and runs bound to other profiles are untouched.
    let closed = any_session_for(&sessions, &a.profile_id);
    sessions.retain(|_, s| s.profile_id != a.profile_id);
    let killed_runs: Vec<Run> = {
        let keys: Vec<String> = runs
            .iter()
            .filter(|(_, r)| r.profile_id == a.profile_id)
            .map(|(k, _)| k.clone())
            .collect();
        keys.iter().filter_map(|k| runs.remove(k)).collect()
    };
    for run in &killed_runs {
        disclose_run_halted(
            state,
            &run.profile_id,
            &run.playlist_id,
            run.position,
            "revocation",
            None,
            now,
        )?;
    }
    let deleted = state.store.delete(&a.profile_id);
    let removed_playlists = state.store.delete_profile_playlists(&a.profile_id)?;
    for playlist_id in &removed_playlists {
        state.store.append_disclosure(&DisclosureRecord {
            at_unix_s: now,
            event: DisclosureEvent::PlaylistDeleted,
            profile_id: a.profile_id.as_str(),
            detail: json!({"playlist_id": playlist_id}),
        })?;
    }
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
    let mut sessions = state.sessions()?;
    let session = require_session(&mut sessions, &a.profile_id, a.session_handle.as_deref())?;
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
    let mut sessions = state.sessions()?;
    let session = require_session(&mut sessions, &a.profile_id, a.session_handle.as_deref())?;
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
    let mut sessions = state.sessions()?;
    let session = require_session(&mut sessions, &a.profile_id, a.session_handle.as_deref())?;
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
        let mut sessions = state.sessions()?;
        let session = require_session(&mut sessions, profile_id, Some(handle))?;
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
        let mut sessions = state.sessions()?;
        if let Ok(session) = require_session(&mut sessions, profile_id, Some(handle)) {
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
                let mut sessions = state.sessions()?;
                require_session(&mut sessions, profile_id, Some(handle))?;
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
        "playlists": {
            "schema": canon.playlists.schema,
            "max_entries_per_playlist": canon.playlists.max_entries_per_playlist,
            "max_layers_per_composite": canon.playlists.max_layers_per_composite,
            "max_depth_step": canon.playlists.max_depth_step,
            "layer_roles": canon.playlists.layer_roles,
            "cadence_cap_per_session": canon.effects.cadence_cap_per_session,
        },
        "grants": [Grant::Subliminal, Grant::TriggerPersistence, Grant::Longitudinal],
        "encoding": canon.encoding,
        "engagement_trigger": engagement_trigger_json(canon),
        "never_generated": "sissification and female-dominant content do not exist in this canon",
    })
}

// ── Sessions as tools (1.1.0, playlists SPEC §4.4) ──────────────
//
// Owner ruling 2026-11-27: `session_open` and `session_close`
// are adopted as spec'd in §4.4. Opening on an existing profile
// returns a handle and adds one session to the concurrent map;
// closing is by handle, and a run-bound session's close is the
// run's advance-or-complete act unless `halt: true`.

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionOpenArgs {
    profile_id: ProfileId,
}

/// Open a session on an existing profile (SPEC §4.4). The
/// profile must exist; the session is additional — sessions on
/// one profile, and on many profiles, run concurrently, each
/// with its own cadence counter and engagement list.
fn session_open(state: &Mesmer, arguments: &Value, now: i64) -> MesmerResult<Value> {
    let a: SessionOpenArgs = args(arguments)?;
    if state.store.get(&a.profile_id)?.is_none() {
        return Err(no_profile(&a.profile_id));
    }
    let mut sessions = state.sessions()?;
    let handle = open_session(state, &mut sessions, &a.profile_id, None, now, None, true)?;
    Ok(json!({
        "session_handle": handle,
        "profile_id": a.profile_id,
        "consent_statement": CONSENT_STATEMENT,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionCloseArgs {
    profile_id: ProfileId,
    session_handle: Option<String>,
    /// Run-bound sessions only: stop the run instead of
    /// advancing it (SPEC §4.4; this is the run-stop act — no
    /// separate stop tool exists).
    #[serde(default)]
    halt: bool,
}

/// Close one session by handle (SPEC §4.4). A plain session
/// simply ends. A run-bound session's close advances the run:
/// the next entry's session opens, judged fresh against the
/// profile as it now stands, or the run completes on its final
/// entry. A refused advance halts the run with the refusal
/// recorded; `halt: true` stops the run deliberately.
fn session_close(state: &Mesmer, arguments: &Value, now: i64) -> MesmerResult<Value> {
    let a: SessionCloseArgs = args(arguments)?;
    let mut sessions = state.sessions()?;
    let mut runs = state.runs()?;
    let handle = a
        .session_handle
        .as_deref()
        .ok_or_else(|| refused(gate::invalid_session()))?;
    let run_ref = sessions
        .get(handle)
        .filter(|s| s.profile_id == a.profile_id)
        .map(|s| s.run.clone())
        .ok_or_else(|| refused(gate::invalid_session()))?;
    let session = sessions
        .remove(handle)
        .expect("session presence checked above");
    disclose_closed(state, &session, now)?;
    let Some(run_ref) = run_ref else {
        return Ok(json!({
            "closed": true,
            "profile_id": a.profile_id,
            "session_handle": handle,
            "deliveries": session.deliveries,
        }));
    };
    let key = run_key(&a.profile_id, &run_ref.playlist_id);
    if a.halt {
        runs.remove(&key);
        disclose_run_halted(
            state,
            &a.profile_id,
            &run_ref.playlist_id,
            run_ref.position,
            "user_stop",
            None,
            now,
        )?;
        return Ok(json!({
            "closed": true,
            "profile_id": a.profile_id,
            "session_handle": handle,
            "run": {"playlist_id": run_ref.playlist_id, "halted": true, "position": run_ref.position},
        }));
    }
    // Advance-or-complete: the run's next act is judged fresh.
    let Some(playlist) =
        state
            .store
            .get_playlist(&a.profile_id, &run_ref.playlist_id, &state.canon)?
    else {
        runs.remove(&key);
        return Err(MesmerError::new(
            RefusalCode::PlaylistUnknown.as_str(),
            format!("no playlist {:?}", run_ref.playlist_id),
        ));
    };
    if run_ref.position + 1 >= playlist.entries.len() {
        // Final entry closed: the run completes, and the
        // completion record restates the end state as
        // delivered-as-authored (SPEC §4.5).
        runs.remove(&key);
        state.store.append_disclosure(&DisclosureRecord {
            at_unix_s: now,
            event: DisclosureEvent::PlaylistRunCompleted,
            profile_id: a.profile_id.as_str(),
            detail: json!({
                "playlist_id": playlist.playlist_id,
                "entries_delivered": playlist.entries.len(),
                "end_state": playlist.end_state,
                "end_state_status": "delivered_as_authored",
            }),
        })?;
        return Ok(json!({
            "closed": true,
            "profile_id": a.profile_id,
            "session_handle": handle,
            "run": {
                "playlist_id": playlist.playlist_id,
                "completed": true,
                "entries_delivered": playlist.entries.len(),
                "end_state": playlist.end_state,
                "end_state_status": "delivered_as_authored",
            },
        }));
    }
    let next = run_ref.position + 1;
    let profile = state
        .store
        .get(&a.profile_id)?
        .ok_or_else(|| no_profile(&a.profile_id))?;
    let context = SessionContext {
        now_unix_s: now,
        session_active: true,
    };
    let request = playlist.entry_request(&state.canon, next);
    let mut refusal = match gate::decide(&state.canon, Some(&profile), &context, &request) {
        gate::Decision::Allow => None,
        gate::Decision::Refuse(rs) => rs.into_iter().next(),
    };
    // The programme day cap binds at every entry opening
    // (SPEC §4.3), not only at start.
    if refusal.is_none()
        && playlist.end_state.persistence == EndStatePersistence::Programme
        && let Some(programme) = profile.programme
    {
        let today = deliveries_today(state, &a.profile_id, now)?;
        if today >= u32::from(programme.cadence_cap_per_day) {
            refusal = Some(gate::programme_day_cap_refusal(
                programme.cadence_cap_per_day,
                today,
            ));
        }
    }
    if let Some(refusal) = refusal {
        runs.remove(&key);
        disclose_run_halted(
            state,
            &a.profile_id,
            &run_ref.playlist_id,
            next,
            "refusal",
            Some(&refusal),
            now,
        )?;
        return Ok(json!({
            "closed": true,
            "profile_id": a.profile_id,
            "session_handle": handle,
            "run": {"playlist_id": run_ref.playlist_id, "halted": true, "position": next},
            "run_halted": true,
            "refusal": {"code": refusal.code, "detail": refusal.detail},
        }));
    }
    let next_ref = RunRef {
        playlist_id: run_ref.playlist_id.clone(),
        position: next,
    };
    let new_handle = open_session(
        state,
        &mut sessions,
        &a.profile_id,
        Some(next_ref),
        now,
        Some(&request),
        true,
    )?;
    if let Some(run) = runs.get_mut(&key) {
        run.position = next;
        run.phase = RunPhase::InSession;
    }
    Ok(json!({
        "closed": true,
        "profile_id": a.profile_id,
        "session_handle": handle,
        "next_session_handle": new_handle,
        "run": {
            "playlist_id": run_ref.playlist_id,
            "position": next,
            "phase": "in_session",
            "entry": entry_json(&playlist.entries[next], playlist.entry_persists(&state.canon, next)),
        },
    }))
}

// ── Playlists (1.1.0, playlists SPEC §4.4) ──────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlaylistCreateArgs {
    profile_id: ProfileId,
    playlist: PlaylistDraft,
}

/// Author a playlist (SPEC §2.1, §4.4): structural validation,
/// then the authoring judgement — every entry judged against
/// the profile as it stands, plus the end-state terms. A
/// refused playlist is not stored.
fn playlist_create(state: &Mesmer, arguments: &Value, now: i64) -> MesmerResult<Value> {
    let a: PlaylistCreateArgs = args(arguments)?;
    let sessions = state.sessions()?;
    let Some(profile) = state.store.get(&a.profile_id)? else {
        return Err(no_profile(&a.profile_id));
    };
    // SPEC §0: the authoring refusal is scoped to the
    // playlist's own profile (spec position, one-place edit —
    // the same site as profile_amend's scoping).
    let context = SessionContext {
        now_unix_s: now,
        session_active: any_session_for(&sessions, &a.profile_id),
    };
    gate::decide_playlist_create(&profile, &context).map_err(refused)?;
    drop(sessions);
    let playlist = PlaylistDefinition::create(a.profile_id.clone(), a.playlist, now)?;
    playlist
        .validate(&state.canon)
        .map_err(|e| MesmerError::new(e.code, e.message))?;
    if let Some(first) = gate::judge_playlist(&state.canon, &profile, &context, &playlist)
        .into_iter()
        .next()
    {
        return Err(refused(first));
    }
    if state
        .store
        .get_playlist(&a.profile_id, &playlist.playlist_id, &state.canon)?
        .is_some()
    {
        return Err(MesmerError::new(
            "MESMER-PLAYLIST-EXISTS",
            "a playlist with this id exists on this profile; playlists are immutable — author a new id",
        ));
    }
    // Stored first, then disclosed; if the disclosure cannot be
    // recorded the create fails closed and the definition is
    // removed again (the profile_create pattern).
    state.store.put_playlist(&playlist)?;
    if let Err(e) = state.store.append_disclosure(&DisclosureRecord {
        at_unix_s: now,
        event: DisclosureEvent::PlaylistCreated,
        profile_id: a.profile_id.as_str(),
        detail: json!({
            "playlist_id": playlist.playlist_id,
            "entries": playlist.entries.len(),
            "end_state": playlist.end_state,
        }),
    }) {
        let _ = state
            .store
            .delete_playlist(&a.profile_id, &playlist.playlist_id);
        return Err(e);
    }
    Ok(json!({"playlist": playlist}))
}

/// One entry as a caller sees it: the delivery, its terms, and
/// its build declaration.
fn entry_json(entry: &PlaylistEntry, persists: bool) -> Value {
    let mut value = serde_json::to_value(entry).unwrap_or(Value::Null);
    if let Some(map) = value.as_object_mut() {
        map.insert("persists".to_string(), json!(persists));
    }
    value
}

/// The run view shared by playlist_show and playlist_start.
fn run_json(run: &Run) -> Value {
    json!({
        "playlist_id": run.playlist_id,
        "position": run.position,
        "phase": run.phase,
    })
}

/// The definition plus its run state, if a run is active
/// (SPEC §4.4).
fn playlist_show(state: &Mesmer, arguments: &Value) -> MesmerResult<Value> {
    let a: PlaylistRefArgs = args(arguments)?;
    if state.store.get(&a.profile_id)?.is_none() {
        return Err(no_profile(&a.profile_id));
    }
    let Some(playlist) = state
        .store
        .get_playlist(&a.profile_id, &a.playlist_id, &state.canon)?
    else {
        return Err(MesmerError::new(
            RefusalCode::PlaylistUnknown.as_str(),
            format!("no playlist {:?}", a.playlist_id),
        ));
    };
    let runs = state.runs()?;
    let run = runs.get(&run_key(&a.profile_id, &a.playlist_id));
    Ok(json!({
        "playlist": playlist,
        "run": run.map(run_json),
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlaylistRefArgs {
    profile_id: ProfileId,
    playlist_id: PlaylistId,
}

/// Start a run (SPEC §4.3, §4.4): the start-level judgement
/// against the profile as it now stands, then entry 0's session
/// opens and the run is disclosed.
fn playlist_start(state: &Mesmer, arguments: &Value, now: i64) -> MesmerResult<Value> {
    let a: PlaylistRefArgs = args(arguments)?;
    let mut sessions = state.sessions()?;
    let mut runs = state.runs()?;
    let Some(profile) = state.store.get(&a.profile_id)? else {
        return Err(no_profile(&a.profile_id));
    };
    let Some(playlist) = state
        .store
        .get_playlist(&a.profile_id, &a.playlist_id, &state.canon)?
    else {
        return Err(MesmerError::new(
            RefusalCode::PlaylistUnknown.as_str(),
            format!("no playlist {:?}", a.playlist_id),
        ));
    };
    let key = run_key(&a.profile_id, &a.playlist_id);
    if runs.contains_key(&key) {
        return Err(MesmerError::new(
            RefusalCode::PlaylistState.as_str(),
            "a run of this playlist is already active",
        ));
    }
    let context = SessionContext {
        now_unix_s: now,
        session_active: any_session_for(&sessions, &a.profile_id),
    };
    let today = deliveries_today(state, &a.profile_id, now)?;
    if let gate::Decision::Refuse(refusals) =
        gate::decide_playlist_start(&state.canon, &profile, &context, &playlist, today)
    {
        return Err(refused(
            refusals.into_iter().next().expect("nonempty refusal"),
        ));
    }
    runs.insert(
        key,
        Run {
            profile_id: a.profile_id.clone(),
            playlist_id: a.playlist_id.clone(),
            position: 0,
            phase: RunPhase::InSession,
        },
    );
    state.store.append_disclosure(&DisclosureRecord {
        at_unix_s: now,
        event: DisclosureEvent::PlaylistRunStarted,
        profile_id: a.profile_id.as_str(),
        detail: json!({
            "playlist_id": playlist.playlist_id,
            "entries": playlist.entries.len(),
            "end_state": playlist.end_state,
        }),
    })?;
    let request = playlist.entry_request(&state.canon, 0);
    let run_ref = RunRef {
        playlist_id: a.playlist_id.clone(),
        position: 0,
    };
    let handle = open_session(
        state,
        &mut sessions,
        &a.profile_id,
        Some(run_ref),
        now,
        Some(&request),
        true,
    )?;
    Ok(json!({
        "session_handle": handle,
        "profile_id": a.profile_id,
        "run": {"playlist_id": a.playlist_id, "position": 0, "phase": "in_session"},
        "entry": entry_json(&playlist.entries[0], playlist.entry_persists(&state.canon, 0)),
    }))
}

/// The profile's disclosed deliveries in the UTC day containing
/// `now` — the programme day-cadence input (SPEC §4.3).
fn deliveries_today(state: &Mesmer, profile_id: &ProfileId, now: i64) -> MesmerResult<u32> {
    let day_start = now.div_euclid(86_400) * 86_400;
    state
        .store
        .deliveries_between(profile_id.as_str(), day_start, day_start + 86_400)
}

// ── Composite delivery (1.1.0, playlists SPEC §3, §4.4) ─────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompositeArgs {
    profile_id: ProfileId,
    session_handle: Option<String>,
    composite: CompositeSpec,
}

/// Deliver a composite (SPEC §3.3): composition-validated, then
/// judged once by the composite gate; on success the whole
/// layered payload is written to the disclosure log as one
/// record before it is returned, and exactly one unit of the
/// session's cadence is consumed.
fn composite_deliver(state: &Mesmer, arguments: &Value, now: i64) -> MesmerResult<Value> {
    let a: CompositeArgs = args(arguments)?;
    let mut sessions = state.sessions()?;
    let session = require_session(&mut sessions, &a.profile_id, a.session_handle.as_deref())?;
    let deliveries = session.deliveries;
    a.composite
        .validate(&state.canon)
        .map_err(|e| MesmerError::new(e.code, e.message))?;
    let profile = state
        .store
        .get(&a.profile_id)?
        .ok_or_else(|| no_profile(&a.profile_id))?;
    let context = SessionContext {
        now_unix_s: now,
        session_active: true,
    };
    let union = gate::decide_composite(&state.canon, &profile, &context, &a.composite, deliveries)
        .map_err(|rs| refused(rs.into_iter().next().expect("nonempty refusal")))?;
    let layers: Vec<Value> = a
        .composite
        .layers
        .iter()
        .map(|layer| {
            let pack = state
                .canon
                .theme_pack(&layer.pack_id)
                .expect("validated layer pack exists");
            json!({
                "role": layer.role,
                "pack": theme_pack_payload(
                    &state.canon, pack, a.composite.intensity.get(), a.composite.duration_minutes,
                ),
            })
        })
        .collect();
    let payload = json!({
        "kind": "composite",
        "intensity": a.composite.intensity,
        "duration_minutes": a.composite.duration_minutes,
        "layers": layers,
        "classes": union.classes,
        "subliminal_layer": union.subliminal_layer,
        "trigger_persistence": union.trigger_persistence,
        "programme_scoped": union.programme_scoped,
        "engagement_trigger": engagement_trigger_json(&state.canon),
        "rendering_obligations": [
            "affirmations are rendered at full volume as the primary programme",
            "a subliminal layer is rendered only with its grant and only at its disclosed level",
            "the delivered layers are exactly the layers named in this payload",
        ],
    });
    session.deliveries += 1;
    let record = DisclosureRecord {
        at_unix_s: now,
        event: DisclosureEvent::DeliveryDisclosed,
        profile_id: a.profile_id.as_str(),
        detail: json!({"request": union, "composite": a.composite, "payload": payload}),
    };
    if state.store.append_disclosure(&record).is_err() {
        session.deliveries -= 1;
        return Err(refused(gate::disclosure_failed()));
    }
    Ok(payload)
}
