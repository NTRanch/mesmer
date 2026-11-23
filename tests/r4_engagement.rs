//! The engagement trigger intake (Owner addition, 2026-11-23):
//! three listener-supplied fields on a server-owned session.
//! `v_` validation, `a_` adversarial.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use mesmer::canon::Canon;
use mesmer::error::MesmerError;
use mesmer::store::Store;
use mesmer::tools::{Mesmer, dispatch};
use serde_json::{Value, json};

static SEQ: AtomicU64 = AtomicU64::new(0);
const T0: i64 = 1_790_000_000;

struct Fixture {
    state: Mesmer,
    dir: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fixture() -> Fixture {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("mesmer-engagement-{}-{n}", std::process::id()));
    let state = Mesmer::new(Canon::embedded().unwrap(), Store::open(&dir).unwrap());
    Fixture { state, dir }
}

impl Fixture {
    fn call(&self, name: &str, args: Value) -> Result<Value, MesmerError> {
        dispatch(&self.state, name, &args, T0)
    }

    /// A profile with ceiling 5, so depth 6 is above it.
    fn create(&self, id: &str) -> Value {
        self.call(
            "consent_profile_create",
            json!({"profile_id": id, "profile": {
                "adult_attested": true,
                "allowed_classes": ["relaxation_descent", "fantasy_immersion"],
                "intensity_ceiling": 5, "max_session_minutes": 40}}),
        )
        .expect("create")
    }

    fn respond(&self, id: &str, handle: &Value, fields: Value) -> Result<Value, MesmerError> {
        let mut args = json!({"profile_id": id, "session_handle": handle});
        args.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        self.call("engagement_respond", args)
    }

    fn engagement_records(&self) -> Vec<Value> {
        let text = std::fs::read_to_string(self.dir.join("disclosure.jsonl")).unwrap_or_default();
        text.lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .filter(|r| r["event"] == "engagement_recorded")
            .collect()
    }
}

fn wren(f: &Fixture, handle: &Value, count: u64, depth: u64) -> Result<Value, MesmerError> {
    f.respond(
        "wren",
        handle,
        json!({"listener_name": "Wren", "listen_count": count, "desired_depth": depth}),
    )
}

// ── Validation half ──────────────────────────────────────────────

#[test]
fn v_wellformed_response_is_recorded_and_disclosed() {
    let f = fixture();
    let created = f.create("wren");
    let out = wren(&f, &created["session_handle"], 3, 5).unwrap();
    assert_eq!(out["recorded"], true);
    assert_eq!(out["response"]["listener_name"], "Wren");
    assert_eq!(out["response"]["listen_count"], 3);
    assert_eq!(out["response"]["desired_depth"], 5);
    let records = f.engagement_records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["detail"]["listener_name"], "Wren");
    assert_eq!(records[0]["detail"]["listen_count"], 3);
    assert_eq!(records[0]["detail"]["desired_depth"], 5);
}

#[test]
fn v_trigger_is_canon_level_and_user_visible() {
    let f = fixture();
    let shown = f.call("canon_show", json!({})).unwrap();
    let trigger = &shown["engagement_trigger"];
    assert_eq!(trigger["cue"], "Tell me three things.");
    assert_eq!(
        trigger["fields"],
        json!(["listener_name", "listen_count", "desired_depth"])
    );
    assert_eq!(trigger["depth_scale"], json!([1, 10]));
    assert_eq!(trigger["placement"], "emergence");
}

#[test]
fn v_an_updated_listen_count_is_a_new_response() {
    let f = fixture();
    let created = f.create("wren");
    wren(&f, &created["session_handle"], 3, 4).unwrap();
    let out = wren(&f, &created["session_handle"], 4, 4).unwrap();
    assert_eq!(out["responses_in_session"], 2);
    assert_eq!(f.engagement_records().len(), 2);
}

#[test]
fn v_fresh_consent_after_revoke_records_again() {
    let f = fixture();
    let created = f.create("wren");
    wren(&f, &created["session_handle"], 1, 2).unwrap();
    f.call("consent_revoke", json!({"profile_id": "wren"}))
        .unwrap();
    let recreated = f.create("wren");
    let out = wren(&f, &recreated["session_handle"], 1, 2).unwrap();
    assert_eq!(out["recorded"], true);
    assert_eq!(out["responses_in_session"], 1);
}

// ── Adversarial half ─────────────────────────────────────────────

#[test]
fn a_missing_field_is_refused() {
    let f = fixture();
    let created = f.create("wren");
    let err = f
        .respond(
            "wren",
            &created["session_handle"],
            json!({"listener_name": "Wren", "desired_depth": 3}),
        )
        .unwrap_err();
    assert_eq!(err.code, "invalid_input");
    assert!(f.engagement_records().is_empty());
}

#[test]
fn a_depth_above_the_canon_cap_is_refused() {
    let f = fixture();
    let created = f.create("wren");
    let err = wren(&f, &created["session_handle"], 1, 11).unwrap_err();
    assert_eq!(err.code, "invalid_input");
    assert!(f.engagement_records().is_empty());
}

#[test]
fn a_depth_above_the_profile_ceiling_is_refused_by_the_gate() {
    let f = fixture();
    let created = f.create("wren");
    // Ceiling is 5; depth 6 is inside the canon scale but outside
    // the envelope — the gate refuses, it never clamps silently
    // and never exceeds.
    let err = wren(&f, &created["session_handle"], 1, 6).unwrap_err();
    assert_eq!(err.code, "MESMER-CEILING");
    assert!(f.engagement_records().is_empty());
}

#[test]
fn a_absurd_listen_count_is_refused() {
    let f = fixture();
    let created = f.create("wren");
    let err = wren(&f, &created["session_handle"], 100_001, 3).unwrap_err();
    assert_eq!(err.code, "invalid_input");
    assert!(f.engagement_records().is_empty());
}

#[test]
fn a_nonnumeric_listen_count_is_refused() {
    let f = fixture();
    let created = f.create("wren");
    let err = f
        .respond(
            "wren",
            &created["session_handle"],
            json!({"listener_name": "Wren", "listen_count": "lots", "desired_depth": 3}),
        )
        .unwrap_err();
    assert_eq!(err.code, "invalid_input");
    assert!(f.engagement_records().is_empty());
}

#[test]
fn a_replay_after_revoke_is_refused_and_the_session_stays_closed() {
    let f = fixture();
    let created = f.create("wren");
    wren(&f, &created["session_handle"], 2, 3).unwrap();
    f.call("consent_revoke", json!({"profile_id": "wren"}))
        .unwrap();
    let err = wren(&f, &created["session_handle"], 2, 3).unwrap_err();
    assert_eq!(err.code, "MESMER-SESSION-INVALID");
    assert_eq!(f.engagement_records().len(), 1, "nothing new was recorded");
}

#[test]
fn a_exact_duplicate_is_refused_and_logged_once() {
    let f = fixture();
    let created = f.create("wren");
    wren(&f, &created["session_handle"], 2, 3).unwrap();
    let err = wren(&f, &created["session_handle"], 2, 3).unwrap_err();
    assert_eq!(err.code, "MESMER-ENGAGEMENT-DUPLICATE");
    assert_eq!(f.engagement_records().len(), 1);
}

#[test]
fn a_blank_listener_name_is_refused() {
    let f = fixture();
    let created = f.create("wren");
    let err = f
        .respond(
            "wren",
            &created["session_handle"],
            json!({"listener_name": "   ", "listen_count": 1, "desired_depth": 3}),
        )
        .unwrap_err();
    assert_eq!(err.code, "invalid_input");
    assert!(f.engagement_records().is_empty());
}
