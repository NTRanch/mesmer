//! The R2 tool surface as plain functions: arguments in, JSON or a
//! typed error out. The clock is an argument (`now_unix_s`), so
//! every path here is testable without wall time; the server reads
//! the clock once per call and passes it down.
//!
//! Lifecycle writes follow validate → gate → commit → record:
//! a refused or invalid call leaves the store unchanged. Revocation
//! is the exception by design: the profile is deleted first (the
//! user's withdrawal takes effect even if the log write fails),
//! then the record is appended.

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

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
    pub canon: Canon,
    pub store: Store,
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
    in_session: bool,
    profile: ProfileDraft,
}

fn profile_create(state: &Mesmer, arguments: &Value, now: i64) -> MesmerResult<Value> {
    let a: CreateArgs = args(arguments)?;
    let context = SessionContext {
        now_unix_s: now,
        session_active: a.in_session,
    };
    gate::decide_create(&context).map_err(refused)?;
    if state.store.get(&a.profile_id)?.is_some() {
        return Err(MesmerError::new(
            "MESMER-PROFILE-EXISTS",
            "a profile with this id exists; amend it or revoke it first",
        ));
    }
    let profile = ConsentProfile::create(a.profile_id, a.profile, now)?;
    state.store.put(&profile)?;
    state.store.append_disclosure(&DisclosureRecord {
        at_unix_s: now,
        event: DisclosureEvent::ProfileCreated,
        profile_id: profile.profile_id.as_str(),
        detail: json!({"revision": profile.revision, "envelope": profile.envelope()}),
    })?;
    Ok(json!({
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
    in_session: bool,
    changes: Amendment,
}

fn profile_amend(state: &Mesmer, arguments: &Value, now: i64) -> MesmerResult<Value> {
    let a: AmendArgs = args(arguments)?;
    let Some(current) = state.store.get(&a.profile_id)? else {
        return Err(no_profile(&a.profile_id));
    };
    let context = SessionContext {
        now_unix_s: now,
        session_active: a.in_session,
    };
    let candidate = current.amended(&a.changes, now)?;
    let kind = gate::decide_amend(&current, &candidate, &context).map_err(refused)?;
    state.store.put(&candidate)?;
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
    let existed = state.store.delete(&a.profile_id)?;
    if existed {
        state.store.append_disclosure(&DisclosureRecord {
            at_unix_s: now,
            event: DisclosureEvent::ProfileRevoked,
            profile_id: a.profile_id.as_str(),
            detail: json!({"total": true}),
        })?;
    }
    Ok(json!({"revoked": existed, "profile_id": a.profile_id}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckArgs {
    profile_id: ProfileId,
    request: SessionRequest,
}

fn envelope_check(state: &Mesmer, arguments: &Value, now: i64) -> MesmerResult<Value> {
    let a: CheckArgs = args(arguments)?;
    if a.request.duration_minutes == 0 {
        return Err(MesmerError::invalid_input("duration_minutes must be >= 1"));
    }
    let profile = state.store.get(&a.profile_id)?;
    // A preview judges a request, not a lifecycle change: whether a
    // session is active does not alter any request rule.
    let context = SessionContext {
        now_unix_s: now,
        session_active: false,
    };
    let decision = gate::decide(&state.canon, profile.as_ref(), &context, &a.request);
    Ok(json!({"decision": decision, "judged_at_unix_s": now}))
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
        "closed": true,
        "intensity_scale": [INTENSITY_MIN, INTENSITY_MAX],
        "classes": classes,
        "techniques": canon.effects.technique,
        "grants": [Grant::Subliminal, Grant::TriggerPersistence, Grant::Longitudinal],
        "encoding": canon.encoding,
        "never_generated": "sissification and female-dominant content do not exist in this canon",
    })
}
