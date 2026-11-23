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

use crate::canon::{Canon, EffectClass, Grant, INTENSITY_MAX, INTENSITY_MIN};
use crate::consent::{
    Amendment, CONSENT_STATEMENT, ConsentProfile, ProfileDraft, ProfileId, SessionContext,
    SessionRequest,
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
}

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
pub const TOOL_NAMES: [&str; 6] = [
    "consent_profile_create",
    "consent_profile_get",
    "consent_profile_amend",
    "consent_revoke",
    "envelope_check",
    "canon_show",
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
    state.store.append_disclosure(&DisclosureRecord {
        at_unix_s: now,
        event: DisclosureEvent::ProfileCreated,
        profile_id: profile.profile_id.as_str(),
        detail: json!({"revision": profile.revision, "envelope": profile.envelope()}),
    })?;
    state.store.put(&profile)?;
    *session = Some(Session {
        profile_id: profile.profile_id.clone(),
        handle: handle.clone(),
        deliveries: 0,
        open: watch::channel(()).0,
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
        "never_generated": "sissification and female-dominant content do not exist in this canon",
    })
}
