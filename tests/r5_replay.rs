//! The conditional replay trigger (Owner addition, 2026-11-23):
//! issued only for an enjoying, 18+-attested, female-attested
//! listener, from her own engagement response. `v_` validation,
//! `a_` adversarial.

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
        .join(format!("mesmer-replay-{}-{n}", std::process::id()));
    let state = Mesmer::new(Canon::embedded().unwrap(), Store::open(&dir).unwrap());
    Fixture { state, dir }
}

impl Fixture {
    fn call(&self, name: &str, args: Value) -> Result<Value, MesmerError> {
        dispatch(&self.state, name, &args, T0)
    }

    fn create(&self, id: &str) -> Value {
        self.call(
            "consent_profile_create",
            json!({"profile_id": id, "profile": {
                "adult_attested": true,
                "allowed_classes": ["identity_affirmation", "fantasy_immersion"],
                "intensity_ceiling": 10, "max_session_minutes": 60}}),
        )
        .expect("create")
    }

    fn respond(&self, id: &str, handle: &Value, extra: Value) -> Result<Value, MesmerError> {
        let mut args = json!({"profile_id": id, "session_handle": handle,
                              "listener_name": "Wren", "listen_count": 2,
                              "desired_depth": 6});
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        self.call("engagement_respond", args)
    }

    fn records(&self, event: &str) -> Vec<Value> {
        let text = std::fs::read_to_string(self.dir.join("disclosure.jsonl")).unwrap_or_default();
        text.lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .filter(|r| r["event"] == event)
            .collect()
    }
}

const QUALIFYING: fn() -> Value =
    || json!({"enjoyed": true, "adult_attested": true, "sex": "female"});

// ── Validation half ──────────────────────────────────────────────

#[test]
fn v_qualifying_response_issues_the_replay_trigger() {
    let f = fixture();
    let created = f.create("wren");
    let out = f
        .respond("wren", &created["session_handle"], QUALIFYING())
        .unwrap();
    assert_eq!(out["recorded"], true);
    assert_eq!(out["replay_trigger"]["issued"], true);
    assert_eq!(out["replay_trigger"]["cue"], "One more time.");
    let issued = f.records("replay_trigger_issued");
    assert_eq!(issued.len(), 1);
    assert_eq!(issued[0]["detail"]["listener_name"], "Wren");
    assert_eq!(issued[0]["detail"]["cue"], "One more time.");
}

#[test]
fn v_replay_spec_is_disclosed_with_its_conditions() {
    let f = fixture();
    let shown = f.call("canon_show", json!({})).unwrap();
    let replay = &shown["engagement_trigger"]["replay"];
    assert_eq!(replay["conditions"]["min_age"], 18);
    assert_eq!(replay["conditions"]["audience"], "adult_female");
    assert_eq!(replay["conditions"]["enjoyment_required"], true);
    // And it rides in a delivery payload, disclosed like
    // everything else in the pack.
    let packs = f.call("theme_pack_show", json!({})).unwrap();
    assert_eq!(
        packs["engagement_trigger"]["replay"]["conditions"]["min_age"],
        18
    );
}

#[test]
fn v_canon_pins_the_hard_age_gate() {
    let canon = Canon::embedded().unwrap();
    assert_eq!(canon.engagement.replay_trigger.min_age, 18);
    assert!(canon.engagement.replay_trigger.requires_enjoyment);
}

// ── Adversarial half ─────────────────────────────────────────────

#[test]
fn a_under_18_attestation_gets_no_trigger() {
    let f = fixture();
    let created = f.create("wren");
    let out = f
        .respond(
            "wren",
            &created["session_handle"],
            json!({"enjoyed": true, "adult_attested": false, "sex": "female"}),
        )
        .unwrap();
    assert!(out["replay_trigger"].is_null());
    assert!(f.records("replay_trigger_issued").is_empty());
}

#[test]
fn a_missing_attestation_gets_no_trigger() {
    let f = fixture();
    let created = f.create("wren");
    // Enjoyment reported, but neither attestation supplied.
    let out = f
        .respond("wren", &created["session_handle"], json!({"enjoyed": true}))
        .unwrap();
    assert!(out["replay_trigger"].is_null());
    assert!(f.records("replay_trigger_issued").is_empty());
}

#[test]
fn a_nonfemale_attestation_gets_no_trigger() {
    let f = fixture();
    let created = f.create("wren");
    for sex in ["male", "other"] {
        let mut extra = QUALIFYING();
        extra["sex"] = json!(sex);
        // Distinct listen counts keep the responses non-duplicate.
        extra["listen_count"] = json!(3);
        let mut args_extra = extra.clone();
        args_extra.as_object_mut().unwrap().remove("listen_count");
        let out = f
            .respond("wren", &created["session_handle"], args_extra)
            .unwrap();
        assert!(out["replay_trigger"].is_null(), "sex={sex} must not issue");
    }
    assert!(f.records("replay_trigger_issued").is_empty());
}

#[test]
fn a_no_enjoyment_gets_no_trigger() {
    let f = fixture();
    let created = f.create("wren");
    let out = f
        .respond(
            "wren",
            &created["session_handle"],
            json!({"enjoyed": false, "adult_attested": true, "sex": "female"}),
        )
        .unwrap();
    assert!(out["replay_trigger"].is_null());
    assert!(f.records("replay_trigger_issued").is_empty());
}

#[test]
fn a_trigger_attempt_on_a_revoked_session_stays_closed() {
    let f = fixture();
    let created = f.create("wren");
    f.respond("wren", &created["session_handle"], QUALIFYING())
        .unwrap();
    assert_eq!(f.records("replay_trigger_issued").len(), 1);
    f.call("consent_revoke", json!({"profile_id": "wren"}))
        .unwrap();
    let err = f
        .respond("wren", &created["session_handle"], QUALIFYING())
        .unwrap_err();
    assert_eq!(err.code, "MESMER-SESSION-INVALID");
    assert_eq!(
        f.records("replay_trigger_issued").len(),
        1,
        "the revoked session issued nothing more"
    );
}
