//! R6 (1.1.0): playlists, composite delivery, and concurrent
//! sessions — the playlists SPEC's §5-named behaviours over a
//! temp data dir. `v_` validation, `a_` adversarial.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use mesmer::canon::{Canon, CanonSources};
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

fn temp_dir(tag: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("{tag}-{}-{n}", std::process::id()))
}

fn fixture() -> Fixture {
    let dir = temp_dir("mesmer-r6");
    let state = Mesmer::new(Canon::embedded().unwrap(), Store::open(&dir).unwrap());
    Fixture { state, dir }
}

/// A fixture whose canon carries one extra test-only pack:
/// a canon-valid amnesia pack (session-only, non-persisting) —
/// the session-only compounding witness (SPEC §5 a7). Canon
/// itself is untouched; this pack exists only in this test's
/// canon instance.
fn amnesia_fixture() -> Fixture {
    const AMNESIA_PACK: &str = r#"
[[pack]]
id = "amnesia_descent"
label = "Amnesia descent — a soft forgetting, session only"
classes = ["amnesia_framing"]
max_intensity = 6
requires_grants = []
trigger_persistence = false
programme_scoped = false

[pack.disclosure]
summary = "Test-only pack: amnesia framing carrying a persistence claim, for scope tests."
contents = ["amnesia framing suggestions"]
never_for_driving = true
stop_cue_required = true

[[pack.affirmation_sets]]
id = "amnesia_descent.first_person"
version = 1
voice = "first_person"
tense = "present"
cue_phrase = "Let it go."
provenance = "estate_original"
lines = ["The edges of the hour soften and blur."]
"#;
    let dir = temp_dir("mesmer-r6-amnesia");
    let themes = format!("{}\n{AMNESIA_PACK}", CanonSources::embedded().themes);
    let canon = Canon::load(&CanonSources {
        themes: &themes,
        ..CanonSources::embedded()
    })
    .expect("test canon loads");
    let state = Mesmer::new(canon, Store::open(&dir).unwrap());
    Fixture { state, dir }
}

const ALL_CLASSES: [&str; 11] = [
    "relaxation_descent",
    "sensory_amplification",
    "sensory_creation",
    "arousal_shaping",
    "orgasm_control",
    "identity_affirmation",
    "submission_headspace",
    "fantasy_immersion",
    "trigger_installation",
    "amnesia_framing",
    "accumulation_programme",
];

fn full_profile(cadence_cap_per_day: u32) -> Value {
    json!({
        "adult_attested": true,
        "allowed_classes": ALL_CLASSES,
        "exclusions": [],
        "intensity_ceiling": 10,
        "max_session_minutes": 120,
        "grants": {"subliminal": true, "trigger_persistence": true, "longitudinal": true},
        "programme": {"expires_in_days": 28, "cadence_cap_per_day": cadence_cap_per_day},
    })
}

impl Fixture {
    fn call(&self, name: &str, args: Value) -> Result<Value, MesmerError> {
        dispatch(&self.state, name, &args, T0)
    }

    fn create(&self, id: &str, profile: Value) -> Value {
        self.call(
            "consent_profile_create",
            json!({"profile_id": id, "profile": profile}),
        )
        .expect("create")
    }

    /// Create a full profile and close the session creation
    /// opened, leaving the profile session-free for authoring.
    fn create_quiet(&self, id: &str, cadence_cap_per_day: u32) {
        let created = self.create(id, full_profile(cadence_cap_per_day));
        self.close(id, &created["session_handle"]);
    }

    fn open(&self, id: &str) -> Value {
        self.call("session_open", json!({"profile_id": id}))
            .expect("session_open")["session_handle"]
            .clone()
    }

    fn close(&self, id: &str, handle: &Value) -> Value {
        self.call(
            "session_close",
            json!({"profile_id": id, "session_handle": handle}),
        )
        .expect("session_close")
    }

    fn deliver(
        &self,
        id: &str,
        handle: &Value,
        pack: &str,
        intensity: u32,
    ) -> Result<Value, MesmerError> {
        self.call(
            "theme_pack_deliver",
            json!({"profile_id": id, "session_handle": handle, "pack_id": pack,
                   "intensity": intensity, "duration_minutes": 20}),
        )
    }

    fn disclosure(&self) -> Vec<Value> {
        let text = std::fs::read_to_string(self.dir.join("disclosure.jsonl")).unwrap_or_default();
        text.lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn events(&self, event: &str) -> Vec<Value> {
        self.disclosure()
            .into_iter()
            .filter(|r| r["event"] == event)
            .collect()
    }

    fn author(
        &self,
        id: &str,
        playlist_id: &str,
        entries: Value,
        end_state: Value,
    ) -> Result<Value, MesmerError> {
        self.call(
            "playlist_create",
            json!({"profile_id": id, "playlist": {
                "playlist_id": playlist_id, "entries": entries, "end_state": end_state}}),
        )
    }

    fn start(&self, id: &str, playlist_id: &str) -> Result<Value, MesmerError> {
        self.call(
            "playlist_start",
            json!({"profile_id": id, "playlist_id": playlist_id}),
        )
    }

    fn show(&self, id: &str, playlist_id: &str) -> Result<Value, MesmerError> {
        self.call(
            "playlist_show",
            json!({"profile_id": id, "playlist_id": playlist_id}),
        )
    }
}

fn pack_entry(pack: &str, intensity: u32, build: Value) -> Value {
    let mut entry = json!({
        "delivery": {"kind": "pack", "pack_id": pack},
        "intensity": intensity, "duration_minutes": 20,
    });
    if !build.is_null() {
        entry["build"] = build;
    }
    entry
}

fn build(builds_on: Value, depth_target: u32, anchors: Value) -> Value {
    json!({"builds_on": builds_on, "depth_target": depth_target, "anchors": anchors})
}

/// A composite entry: a persisting bed (submissive training)
/// layered with the session-only amnesia pack — the union
/// claims persistence over a session-only class.
fn amnesia_composite_entry(build: Value) -> Value {
    let mut entry = json!({
        "delivery": {"kind": "composite", "composite": {
            "intensity": 3, "duration_minutes": 20,
            "layers": [
                {"role": "bed", "pack_id": "submissive_training"},
                {"role": "visual", "pack_id": "amnesia_descent"},
            ]}},
        "intensity": 3, "duration_minutes": 20,
    });
    if !build.is_null() {
        entry["build"] = build;
    }
    entry
}

fn end_state(classes: Value, depth_target: u32, persistence: &str, anchors: Value) -> Value {
    json!({"classes": classes, "depth_target": depth_target,
           "persistence": persistence, "anchors": anchors})
}

// ── Validation half ──────────────────────────────────────────────

#[test]
fn v1_three_entry_chain_runs_with_a_session_per_entry() {
    let f = fixture();
    f.create_quiet("a", 8);
    f.author(
        "a",
        "chain",
        json!([
            pack_entry("bimbo_glamour", 3, build(json!([]), 5, json!([]))),
            pack_entry("bimbo_glamour", 4, Value::Null),
            pack_entry("bimbo_glamour", 5, build(json!([]), 5, json!([]))),
        ]),
        end_state(json!(["identity_affirmation"]), 5, "run_only", json!([])),
    )
    .expect("author");
    let started = f.start("a", "chain").expect("start");
    let mut handles = vec![started["session_handle"].clone()];
    for _ in 0..2 {
        let handle = handles.last().unwrap().clone();
        f.deliver("a", &handle, "bimbo_glamour", 3)
            .expect("deliver");
        let closed = f.close("a", &handle);
        handles.push(closed["next_session_handle"].clone());
    }
    let last = handles.last().unwrap().clone();
    f.deliver("a", &last, "bimbo_glamour", 3).expect("deliver");
    let closed = f.close("a", &last);
    assert_eq!(closed["run"]["completed"], true);
    // Three entries, three distinct sessions, one delivery each.
    let unique: std::collections::HashSet<&Value> = handles.iter().collect();
    assert_eq!(unique.len(), 3);
    assert_eq!(f.events("delivery_disclosed").len(), 3);
    assert_eq!(f.events("playlist_entry_opened").len(), 3);
    assert_eq!(f.events("playlist_run_completed").len(), 1);
    assert!(f.show("a", "chain").unwrap()["run"].is_null());
}

#[test]
fn v2_entry_session_keeps_the_single_delivery_cap() {
    let f = fixture();
    f.create_quiet("a", 8);
    f.author(
        "a",
        "one",
        json!([pack_entry(
            "bimbo_glamour",
            3,
            build(json!([]), 3, json!([]))
        )]),
        end_state(json!(["identity_affirmation"]), 3, "run_only", json!([])),
    )
    .expect("author");
    let handle = f.start("a", "one").expect("start")["session_handle"].clone();
    f.deliver("a", &handle, "bimbo_glamour", 3).expect("first");
    let err = f.deliver("a", &handle, "bimbo_glamour", 3).unwrap_err();
    assert_eq!(err.code, "MESMER-CADENCE-CAP");
    assert_eq!(f.events("delivery_disclosed").len(), 1);
}

#[test]
fn v3_composite_is_one_delivery_one_record_all_layers() {
    let f = fixture();
    f.create_quiet("a", 8);
    let handle = f.open("a");
    let payload = f
        .call(
            "composite_deliver",
            json!({"profile_id": "a", "session_handle": handle, "composite": {
            "intensity": 5, "duration_minutes": 20,
            "layers": [
                {"role": "bed", "pack_id": "submissive_training"},
                {"role": "visual", "pack_id": "bimbo_glamour"},
                {"role": "drone", "pack_id": "bimbo_glamour"},
            ]}}),
        )
        .expect("composite");
    assert_eq!(payload["kind"], "composite");
    assert_eq!(payload["layers"].as_array().unwrap().len(), 3);
    let classes: Vec<&str> = payload["classes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c.as_str().unwrap())
        .collect();
    assert!(classes.contains(&"submission_headspace"));
    assert!(classes.contains(&"fantasy_immersion"));
    // Exactly one disclosure record, carrying the whole payload.
    let deliveries = f.events("delivery_disclosed");
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0]["detail"]["payload"], payload);
    // And exactly one unit of cadence: the session is spent.
    let err = f.deliver("a", &handle, "bimbo_glamour", 3).unwrap_err();
    assert_eq!(err.code, "MESMER-CADENCE-CAP");
}

#[test]
fn v4_unreachable_end_state_refuses_at_authoring() {
    let f = fixture();
    f.create_quiet("a", 8);
    let err = f
        .author(
            "a",
            "unreachable",
            json!([pack_entry(
                "bimbo_glamour",
                3,
                build(json!([]), 3, json!([]))
            )]),
            end_state(json!(["orgasm_control"]), 3, "run_only", json!([])),
        )
        .unwrap_err();
    assert_eq!(err.code, "MESMER-PLAYLIST-INVALID");
    // Nothing was stored.
    let err = f.show("a", "unreachable").unwrap_err();
    assert_eq!(err.code, "MESMER-PLAYLIST-UNKNOWN");
}

#[test]
fn v5_monotonic_programme_chain_with_anchors_creates_and_runs() {
    let f = fixture();
    f.create_quiet("a", 8);
    f.author(
        "a",
        "training",
        json!([
            pack_entry("submissive_training", 3, build(json!([]), 3, json!([]))),
            pack_entry(
                "submissive_training",
                5,
                build(json!([0]), 5, json!(["settle now"]))
            ),
            pack_entry(
                "submissive_training",
                8,
                build(json!([0, 1]), 8, json!(["settle now"]))
            ),
        ]),
        end_state(
            json!(["trigger_installation"]),
            8,
            "programme",
            json!(["settle now"]),
        ),
    )
    .expect("author");
    let mut handle = f.start("a", "training").expect("start")["session_handle"].clone();
    for _ in 0..2 {
        f.deliver("a", &handle, "submissive_training", 3)
            .expect("deliver");
        handle = f.close("a", &handle)["next_session_handle"].clone();
    }
    f.deliver("a", &handle, "submissive_training", 3)
        .expect("deliver");
    let closed = f.close("a", &handle);
    assert_eq!(closed["run"]["completed"], true);
    assert_eq!(closed["run"]["end_state"]["depth_target"], 8);
}

#[test]
fn v6_programme_day_cap_halts_at_the_boundary_entry() {
    let f = fixture();
    f.create_quiet("a", 2);
    f.author(
        "a",
        "daily",
        json!([
            pack_entry("submissive_training", 3, build(json!([]), 3, json!([]))),
            pack_entry("submissive_training", 3, Value::Null),
            pack_entry("submissive_training", 3, build(json!([]), 3, json!([]))),
        ]),
        end_state(json!(["trigger_installation"]), 3, "programme", json!([])),
    )
    .expect("author");
    let mut handle = f.start("a", "daily").expect("start")["session_handle"].clone();
    f.deliver("a", &handle, "submissive_training", 3)
        .expect("d1");
    handle = f.close("a", &handle)["next_session_handle"].clone();
    f.deliver("a", &handle, "submissive_training", 3)
        .expect("d2");
    // Two deliveries disclosed today; entry 2 may not open.
    let closed = f.close("a", &handle);
    assert_eq!(closed["run_halted"], true);
    assert_eq!(closed["refusal"]["code"], "MESMER-CADENCE-CAP");
    let halted = f.events("playlist_run_halted");
    assert_eq!(halted.len(), 1);
    assert_eq!(halted[0]["detail"]["position"], 2);
    assert!(f.show("a", "daily").unwrap()["run"].is_null());
}

#[test]
fn v7_returned_composite_equals_the_disclosed_payload() {
    let f = fixture();
    f.create_quiet("a", 8);
    let handle = f.open("a");
    let payload = f
        .call(
            "composite_deliver",
            json!({"profile_id": "a", "session_handle": handle, "composite": {
                "intensity": 4, "duration_minutes": 15,
                "layers": [{"role": "bed", "pack_id": "bimbo_glamour"}]}}),
        )
        .expect("composite");
    let deliveries = f.events("delivery_disclosed");
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0]["detail"]["payload"], payload);
    assert_eq!(
        deliveries[0]["detail"]["composite"]["layers"][0]["pack_id"],
        "bimbo_glamour"
    );
}

#[test]
fn v8_halt_stops_the_run_and_leaves_other_sessions_alone() {
    let f = fixture();
    f.create_quiet("a", 8);
    let other = f.create("b", full_profile(8));
    let other_handle = other["session_handle"].clone();
    f.author(
        "a",
        "two",
        json!([
            pack_entry("bimbo_glamour", 3, build(json!([]), 3, json!([]))),
            pack_entry("bimbo_glamour", 3, build(json!([]), 3, json!([]))),
        ]),
        end_state(json!(["identity_affirmation"]), 3, "run_only", json!([])),
    )
    .expect("author");
    let handle = f.start("a", "two").expect("start")["session_handle"].clone();
    let closed = f
        .call(
            "session_close",
            json!({"profile_id": "a", "session_handle": handle, "halt": true}),
        )
        .expect("halt close");
    assert_eq!(closed["run"]["halted"], true);
    assert!(f.show("a", "two").unwrap()["run"].is_null());
    let halted = f.events("playlist_run_halted");
    assert_eq!(halted.len(), 1);
    assert_eq!(halted[0]["detail"]["reason"], "user_stop");
    // The unrelated session on profile b is untouched.
    f.deliver("b", &other_handle, "bimbo_glamour", 3)
        .expect("other profile still delivers");
}

#[test]
fn v9_restart_after_halt_rejudges_against_the_narrowed_profile() {
    let f = fixture();
    f.create_quiet("a", 8);
    f.author(
        "a",
        "needs_grants",
        json!([
            pack_entry("bimbo_glamour", 3, build(json!([]), 3, json!([]))),
            pack_entry("submissive_training", 3, build(json!([]), 3, json!([]))),
        ]),
        end_state(json!(["identity_affirmation"]), 3, "run_only", json!([])),
    )
    .expect("author");
    let handle = f.start("a", "needs_grants").expect("start")["session_handle"].clone();
    f.call(
        "session_close",
        json!({"profile_id": "a", "session_handle": handle, "halt": true}),
    )
    .expect("halt");
    // Narrowing is always allowed: drop the persistence grant.
    f.call(
        "consent_profile_amend",
        json!({"profile_id": "a", "changes": {"grants": {"trigger_persistence": false}}}),
    )
    .expect("narrow");
    let err = f.start("a", "needs_grants").unwrap_err();
    assert_eq!(err.code, "MESMER-GRANT-ABSENT");
}

#[test]
fn v10_session_open_close_lifecycle_on_an_existing_profile() {
    let f = fixture();
    let created = f.create("a", full_profile(8));
    let h0 = created["session_handle"].clone();
    let h1 = f.open("a");
    assert_ne!(h1, h0);
    f.deliver("a", &h1, "bimbo_glamour", 3).expect("deliver h1");
    // h0's cadence is its own: it has delivered nothing.
    f.deliver("a", &h0, "bimbo_glamour", 3).expect("deliver h0");
    let closed = f.close("a", &h1);
    assert_eq!(closed["closed"], true);
    assert_eq!(closed["deliveries"], 1);
    let h2 = f.open("a");
    assert_ne!(h2, h0);
    assert_ne!(h2, h1);
    assert_eq!(f.events("session_opened").len(), 2);
    f.close("a", &h0);
    f.close("a", &h2);
    assert_eq!(f.events("session_closed").len(), 3);
}

#[test]
fn v11_anchor_continuity_follows_the_installing_entry() {
    let f = fixture();
    f.create_quiet("a", 8);
    f.author(
        "a",
        "anchored",
        json!([
            pack_entry("submissive_training", 3, build(json!([]), 3, json!([]))),
            pack_entry(
                "submissive_training",
                3,
                build(json!([0]), 3, json!(["settle now"]))
            ),
        ]),
        end_state(
            json!(["identity_affirmation"]),
            3,
            "run_only",
            json!(["settle now"]),
        ),
    )
    .expect("anchor installed by entry 0 carries into entry 1");
    // The same build on a chain whose first entry installs
    // nothing refuses: the cue is not the chain's to name.
    let err = f
        .author(
            "a",
            "unanchored",
            json!([
                pack_entry("bimbo_glamour", 3, build(json!([]), 3, json!([]))),
                pack_entry(
                    "bimbo_glamour",
                    3,
                    build(json!([0]), 3, json!(["settle now"]))
                ),
            ]),
            end_state(json!(["identity_affirmation"]), 3, "run_only", json!([])),
        )
        .unwrap_err();
    assert_eq!(err.code, "MESMER-PLAYLIST-INVALID");
}

#[test]
fn v12_completion_record_restates_the_end_state() {
    let f = fixture();
    f.create_quiet("a", 8);
    let end = end_state(json!(["identity_affirmation"]), 4, "run_only", json!([]));
    f.author(
        "a",
        "once",
        json!([pack_entry(
            "bimbo_glamour",
            4,
            build(json!([]), 4, json!([]))
        )]),
        end.clone(),
    )
    .expect("author");
    let handle = f.start("a", "once").expect("start")["session_handle"].clone();
    f.close("a", &handle);
    let completed = f.events("playlist_run_completed");
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0]["detail"]["entries_delivered"], 1);
    assert_eq!(completed[0]["detail"]["end_state"], end);
    assert_eq!(
        completed[0]["detail"]["end_state_status"],
        "delivered_as_authored"
    );
}

#[test]
fn v13_two_profiles_runs_and_a_standalone_session_are_independent() {
    let f = fixture();
    f.create_quiet("a", 8);
    f.create_quiet("b", 8);
    for id in ["a", "b"] {
        f.author(
            id,
            "solo",
            json!([pack_entry(
                "bimbo_glamour",
                3,
                build(json!([]), 3, json!([]))
            )]),
            end_state(json!(["identity_affirmation"]), 3, "run_only", json!([])),
        )
        .expect("author");
    }
    let ha = f.start("a", "solo").expect("start a")["session_handle"].clone();
    let hb = f.start("b", "solo").expect("start b")["session_handle"].clone();
    let standalone = f.open("a");
    assert_ne!(ha, hb);
    f.deliver("a", &ha, "bimbo_glamour", 3).expect("a entry");
    f.deliver("b", &hb, "bimbo_glamour", 3).expect("b entry");
    f.deliver("a", &standalone, "bimbo_glamour", 3)
        .expect("standalone");
    let closed_a = f.close("a", &ha);
    assert_eq!(closed_a["run"]["completed"], true);
    // b's run is still active and completes on its own close.
    assert!(f.show("b", "solo").unwrap()["run"].is_object());
    let closed_b = f.close("b", &hb);
    assert_eq!(closed_b["run"]["completed"], true);
    assert_eq!(f.events("playlist_run_completed").len(), 2);
}

// ── Adversarial half ─────────────────────────────────────────────

#[test]
fn a1_a_replayed_entry_handle_is_dead() {
    let f = fixture();
    f.create_quiet("a", 8);
    f.author(
        "a",
        "once",
        json!([pack_entry(
            "bimbo_glamour",
            3,
            build(json!([]), 3, json!([]))
        )]),
        end_state(json!(["identity_affirmation"]), 3, "run_only", json!([])),
    )
    .expect("author");
    let handle = f.start("a", "once").expect("start")["session_handle"].clone();
    f.close("a", &handle);
    let err = f
        .call(
            "session_close",
            json!({"profile_id": "a", "session_handle": handle}),
        )
        .unwrap_err();
    assert_eq!(err.code, "MESMER-SESSION-INVALID");
    let err = f.deliver("a", &handle, "bimbo_glamour", 3).unwrap_err();
    assert_eq!(err.code, "MESMER-SESSION-INVALID");
}

#[test]
fn a2_excluded_classes_do_not_exist_anywhere() {
    let f = fixture();
    f.create_quiet("a", 8);
    // The excluded names fail through the same generic
    // unknown-class path as any other non-canon string: they
    // are not special-cased because they are not anything.
    for bogus in ["sissification", "femdom", "not_a_class"] {
        let err = f
            .author(
                "a",
                "bogus",
                json!([pack_entry(
                    "bimbo_glamour",
                    3,
                    build(json!([]), 3, json!([]))
                )]),
                end_state(json!([bogus]), 3, "run_only", json!([])),
            )
            .unwrap_err();
        assert_eq!(err.code, "invalid_input", "{bogus}");
    }
    // And as a composite layer's pack, the name is simply a
    // pack that does not exist.
    let handle = f.open("a");
    let err = f
        .call(
            "composite_deliver",
            json!({"profile_id": "a", "session_handle": handle, "composite": {
                "intensity": 3, "duration_minutes": 20,
                "layers": [{"role": "bed", "pack_id": "sissification"}]}}),
        )
        .unwrap_err();
    assert_eq!(err.code, "MESMER-PLAYLIST-INVALID");
}

#[test]
fn a3_one_out_of_envelope_layer_refuses_the_whole_composite() {
    let f = fixture();
    let mut profile = full_profile(8);
    profile["allowed_classes"] = json!([
        "relaxation_descent",
        "sensory_amplification",
        "sensory_creation",
        "arousal_shaping",
        "orgasm_control",
        "identity_affirmation",
        "submission_headspace",
        "trigger_installation",
        "amnesia_framing",
        "accumulation_programme",
    ]); // no fantasy_immersion: the bimbo layer is out of envelope
    let created = f.create("a", profile);
    let handle = created["session_handle"].clone();
    let err = f
        .call(
            "composite_deliver",
            json!({"profile_id": "a", "session_handle": handle, "composite": {
            "intensity": 5, "duration_minutes": 20,
            "layers": [
                {"role": "bed", "pack_id": "submissive_training"},
                {"role": "visual", "pack_id": "bimbo_glamour"},
            ]}}),
        )
        .unwrap_err();
    assert_eq!(err.code, "MESMER-ENVELOPE-CLASS");
    // No payload, no cadence consumed, no record gap.
    assert_eq!(f.events("delivery_disclosed").len(), 0);
    f.deliver("a", &handle, "submissive_training", 3)
        .expect("the session's cadence is intact");
}

#[test]
fn a4_composition_refusals() {
    let f = fixture();
    f.create_quiet("a", 8);
    let handle = f.open("a");
    let composite = |layers: Value| {
        f.call(
            "composite_deliver",
            json!({"profile_id": "a", "session_handle": handle, "composite": {
                "intensity": 3, "duration_minutes": 20, "layers": layers}}),
        )
        .unwrap_err()
    };
    // Two subliminal layers.
    let err = composite(json!([
        {"role": "bed", "pack_id": "bimbo_glamour"},
        {"role": "subliminal", "pack_id": "submissive_training"},
        {"role": "subliminal", "pack_id": "submissive_training"},
    ]));
    assert_eq!(err.code, "MESMER-PLAYLIST-INVALID");
    // An anchor riding a non-bed layer.
    let err = composite(json!([
        {"role": "bed", "pack_id": "bimbo_glamour"},
        {"role": "visual", "pack_id": "submissive_training"},
    ]));
    assert_eq!(err.code, "MESMER-PLAYLIST-INVALID");
    // Five layers.
    let err = composite(json!([
        {"role": "bed", "pack_id": "bimbo_glamour"},
        {"role": "visual", "pack_id": "bimbo_glamour"},
        {"role": "drone", "pack_id": "bimbo_glamour"},
        {"role": "drone", "pack_id": "bimbo_glamour"},
        {"role": "drone", "pack_id": "bimbo_glamour"},
    ]));
    assert_eq!(err.code, "MESMER-PLAYLIST-INVALID");
    // No bed at all.
    let err = composite(json!([{"role": "drone", "pack_id": "bimbo_glamour"}]));
    assert_eq!(err.code, "MESMER-PLAYLIST-INVALID");
    assert_eq!(f.events("delivery_disclosed").len(), 0);
}

#[test]
fn a5_narrowing_mid_run_halts_the_next_entry() {
    let f = fixture();
    f.create_quiet("a", 8);
    f.author(
        "a",
        "chain",
        json!([
            pack_entry("bimbo_glamour", 3, build(json!([]), 3, json!([]))),
            pack_entry("submissive_training", 3, build(json!([]), 3, json!([]))),
        ]),
        end_state(json!(["identity_affirmation"]), 3, "run_only", json!([])),
    )
    .expect("author");
    let handle = f.start("a", "chain").expect("start")["session_handle"].clone();
    f.deliver("a", &handle, "bimbo_glamour", 3)
        .expect("entry 0");
    // Narrowing is always allowed, even mid-run: entry 1's
    // classes leave the envelope.
    f.call(
        "consent_profile_amend",
        json!({"profile_id": "a", "changes": {
            "allowed_classes": ["identity_affirmation", "fantasy_immersion",
                                "sensory_amplification", "arousal_shaping"]}}),
    )
    .expect("narrow");
    let closed = f.close("a", &handle);
    assert_eq!(closed["run_halted"], true);
    assert_eq!(closed["refusal"]["code"], "MESMER-ENVELOPE-CLASS");
    assert!(f.show("a", "chain").unwrap()["run"].is_null());
}

#[test]
fn a6_revocation_mid_chain_kills_run_sessions_and_playlists() {
    let f = fixture();
    f.create_quiet("a", 8);
    let b = f.create("b", full_profile(8));
    let b_handle = b["session_handle"].clone();
    f.author(
        "a",
        "chain",
        json!([
            pack_entry("bimbo_glamour", 3, build(json!([]), 3, json!([]))),
            pack_entry("bimbo_glamour", 3, build(json!([]), 3, json!([]))),
        ]),
        end_state(json!(["identity_affirmation"]), 3, "run_only", json!([])),
    )
    .expect("author");
    let handle = f.start("a", "chain").expect("start")["session_handle"].clone();
    f.call("consent_revoke", json!({"profile_id": "a"}))
        .expect("revoke");
    let err = f.deliver("a", &handle, "bimbo_glamour", 3).unwrap_err();
    assert_eq!(err.code, "MESMER-SESSION-INVALID");
    let err = f.show("a", "chain").unwrap_err();
    assert_eq!(err.code, "MESMER-NO-PROFILE");
    let halted = f.events("playlist_run_halted");
    assert_eq!(halted.len(), 1);
    assert_eq!(halted[0]["detail"]["reason"], "revocation");
    assert_eq!(f.events("playlist_deleted").len(), 1);
    // Profile b is untouched.
    f.deliver("b", &b_handle, "bimbo_glamour", 3)
        .expect("b still delivers");
}

#[test]
fn a7_session_only_compounding_is_a_scope_refusal() {
    let f = amnesia_fixture();
    f.create_quiet("a", 8);
    // Canon forbids a pack from persisting a session-only class
    // outright (r1 pins that), so the compounding claim arises
    // through composition: a composite whose union persists
    // (its bed) while one layer is session-only.
    // (1) A build declared on such a composite entry.
    let err = f
        .author(
            "a",
            "claim_on_session_only",
            json!([amnesia_composite_entry(build(json!([]), 3, json!([])))]),
            end_state(json!(["identity_affirmation"]), 3, "run_only", json!([])),
        )
        .unwrap_err();
    assert_eq!(err.code, "MESMER-PLAYLIST-SCOPE");
    // (2) Building on a session-only entry.
    let err = f
        .author(
            "a",
            "builds_on_session_only",
            json!([
                pack_entry("amnesia_descent", 3, Value::Null),
                pack_entry("bimbo_glamour", 3, build(json!([0]), 3, json!([]))),
            ]),
            end_state(json!(["identity_affirmation"]), 3, "run_only", json!([])),
        )
        .unwrap_err();
    assert_eq!(err.code, "MESMER-PLAYLIST-SCOPE");
    // (3) The composite building on an honest entry: its own
    // claim still cannot persist.
    let err = f
        .author(
            "a",
            "session_only_builds",
            json!([
                pack_entry("bimbo_glamour", 3, build(json!([]), 3, json!([]))),
                amnesia_composite_entry(build(json!([0]), 3, json!([]))),
            ]),
            end_state(json!(["identity_affirmation"]), 3, "run_only", json!([])),
        )
        .unwrap_err();
    assert_eq!(err.code, "MESMER-PLAYLIST-SCOPE");
}

#[test]
fn a8_authoring_refusal_is_scoped_to_the_playlists_own_profile() {
    let f = fixture();
    f.create_quiet("a", 8);
    f.create_quiet("b", 8);
    let b_session = f.open("b");
    // A session on b does not freeze authoring on a.
    f.author(
        "a",
        "fine",
        json!([pack_entry(
            "bimbo_glamour",
            3,
            build(json!([]), 3, json!([]))
        )]),
        end_state(json!(["identity_affirmation"]), 3, "run_only", json!([])),
    )
    .expect("authoring on a proceeds while b is in session");
    // A session on a itself does.
    let _a_session = f.open("a");
    let err = f
        .author(
            "a",
            "frozen",
            json!([pack_entry(
                "bimbo_glamour",
                3,
                build(json!([]), 3, json!([]))
            )]),
            end_state(json!(["identity_affirmation"]), 3, "run_only", json!([])),
        )
        .unwrap_err();
    assert_eq!(err.code, "MESMER-WIDEN-IN-SESSION");
    drop(b_session);
}

#[test]
fn a9_a_second_run_of_one_playlist_is_a_state_refusal() {
    let f = fixture();
    f.create_quiet("a", 8);
    f.create_quiet("b", 8);
    for id in ["a", "b"] {
        f.author(
            id,
            "solo",
            json!([pack_entry(
                "bimbo_glamour",
                3,
                build(json!([]), 3, json!([]))
            )]),
            end_state(json!(["identity_affirmation"]), 3, "run_only", json!([])),
        )
        .expect("author");
    }
    f.start("a", "solo").expect("first start");
    let err = f.start("a", "solo").unwrap_err();
    assert_eq!(err.code, "MESMER-PLAYLIST-STATE");
    // The other profile's run of its own playlist is unaffected.
    f.start("b", "solo").expect("b starts its own");
}

#[test]
fn a10_end_depth_above_the_ceiling_refuses_at_authoring_and_at_start() {
    let f = fixture();
    // Ceiling 6: an end depth of 8 refuses at authoring.
    let mut low = full_profile(8);
    low["intensity_ceiling"] = json!(6);
    let created = f.create("low", low);
    let handle = created["session_handle"].clone();
    f.close("low", &handle);
    let deep_chain = json!([
        pack_entry("submissive_training", 3, build(json!([]), 3, json!([]))),
        pack_entry("submissive_training", 5, build(json!([0]), 5, json!([]))),
        pack_entry("submissive_training", 8, build(json!([1]), 8, json!([]))),
    ]);
    let err = f
        .author(
            "low",
            "deep",
            deep_chain.clone(),
            end_state(json!(["trigger_installation"]), 8, "run_only", json!([])),
        )
        .unwrap_err();
    assert_eq!(err.code, "MESMER-CEILING");
    // Ceiling 10 at authoring, narrowed to 6 before the start:
    // the start-level check refuses.
    f.create_quiet("a", 8);
    f.author(
        "a",
        "deep",
        deep_chain,
        end_state(json!(["trigger_installation"]), 8, "run_only", json!([])),
    )
    .expect("author at ceiling 10");
    f.call(
        "consent_profile_amend",
        json!({"profile_id": "a", "changes": {"intensity_ceiling": 6}}),
    )
    .expect("narrow the ceiling");
    let err = f.start("a", "deep").unwrap_err();
    assert_eq!(err.code, "MESMER-CEILING");
}

#[test]
fn a11_cross_profile_playlists_and_handles_do_not_resolve() {
    let f = fixture();
    f.create_quiet("a", 8);
    f.create_quiet("b", 8);
    f.author(
        "a",
        "mine",
        json!([pack_entry(
            "bimbo_glamour",
            3,
            build(json!([]), 3, json!([]))
        )]),
        end_state(json!(["identity_affirmation"]), 3, "run_only", json!([])),
    )
    .expect("author");
    // b cannot start (or even see) a's playlist.
    let err = f.start("b", "mine").unwrap_err();
    assert_eq!(err.code, "MESMER-PLAYLIST-UNKNOWN");
    // a's handle presented against b refuses as any forgery.
    let handle = f.start("a", "mine").expect("start")["session_handle"].clone();
    let err = f.deliver("b", &handle, "bimbo_glamour", 3).unwrap_err();
    assert_eq!(err.code, "MESMER-SESSION-INVALID");
}

#[test]
fn a12_a_disclosed_composite_cannot_be_redelivered_or_altered() {
    let f = fixture();
    f.create_quiet("a", 8);
    let handle = f.open("a");
    let args = json!({"profile_id": "a", "session_handle": handle, "composite": {
        "intensity": 4, "duration_minutes": 15,
        "layers": [{"role": "bed", "pack_id": "bimbo_glamour"}]}});
    let payload = f.call("composite_deliver", args.clone()).expect("first");
    // The same call again is not a second delivery: the
    // session's one cadence unit is spent.
    let err = f.call("composite_deliver", args).unwrap_err();
    assert_eq!(err.code, "MESMER-CADENCE-CAP");
    let deliveries = f.events("delivery_disclosed");
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0]["detail"]["payload"], payload);
}

#[test]
fn a13_concurrent_sessions_do_not_feed_a_playlists_compounding() {
    let f = fixture();
    f.create_quiet("a", 8);
    // A standalone session on the same profile really installs
    // the anchor — in its own session.
    let standalone = f.open("a");
    f.deliver("a", &standalone, "submissive_training", 3)
        .expect("standalone delivery installs the anchor there");
    f.close("a", &standalone);
    // The playlist's compounding reads only its own entries:
    // entry 0 (bimbo) installs nothing, so entry 1 may not
    // name the cue, whatever a concurrent session did.
    let err = f
        .author(
            "a",
            "borrowed_anchor",
            json!([
                pack_entry("bimbo_glamour", 3, build(json!([]), 3, json!([]))),
                pack_entry(
                    "bimbo_glamour",
                    3,
                    build(json!([0]), 3, json!(["settle now"]))
                ),
            ]),
            end_state(json!(["identity_affirmation"]), 3, "run_only", json!([])),
        )
        .unwrap_err();
    assert_eq!(err.code, "MESMER-PLAYLIST-INVALID");
    // And a handle from one session never addresses another
    // profile's session, even mid-run.
    f.create_quiet("b", 8);
    let a_session = f.open("a");
    let err = f.deliver("b", &a_session, "bimbo_glamour", 3).unwrap_err();
    assert_eq!(err.code, "MESMER-SESSION-INVALID");
}
