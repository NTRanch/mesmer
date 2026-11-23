//! Production theme packs: canon shape, the catalogue, and
//! gated disclose-before-deliver delivery. `v_` validation,
//! `a_` adversarial.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use mesmer::canon::{Canon, CanonSources, Provenance};
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
        .join(format!("mesmer-themes-{}-{n}", std::process::id()));
    let state = Mesmer::new(Canon::embedded().unwrap(), Store::open(&dir).unwrap());
    Fixture { state, dir }
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

    fn disclosure(&self) -> Vec<Value> {
        let text = std::fs::read_to_string(self.dir.join("disclosure.jsonl")).unwrap_or_default();
        text.lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn deliveries(&self) -> Vec<Value> {
        self.disclosure()
            .into_iter()
            .filter(|r| r["event"] == "delivery_disclosed")
            .collect()
    }
}

fn bimbo_profile() -> Value {
    json!({"adult_attested": true,
           "allowed_classes": ["identity_affirmation", "fantasy_immersion",
                               "sensory_amplification", "arousal_shaping"],
           "intensity_ceiling": 10, "max_session_minutes": 60})
}

fn full_profile() -> Value {
    json!({"adult_attested": true,
           "allowed_classes": ["relaxation_descent", "sensory_amplification", "sensory_creation",
                               "arousal_shaping", "orgasm_control", "identity_affirmation",
                               "submission_headspace", "fantasy_immersion", "trigger_installation",
                               "amnesia_framing", "accumulation_programme"],
           "intensity_ceiling": 10, "max_session_minutes": 180,
           "grants": {"subliminal": true, "trigger_persistence": true, "longitudinal": true},
           "programme": {"expires_in_days": 28, "cadence_cap_per_day": 8}})
}

fn deliver(
    f: &Fixture,
    id: &str,
    handle: &Value,
    pack: &str,
    intensity: u64,
) -> Result<Value, MesmerError> {
    f.call(
        "theme_pack_deliver",
        json!({"profile_id": id, "session_handle": handle, "pack_id": pack,
               "intensity": intensity, "duration_minutes": 30}),
    )
}

fn load_with_themes(themes: &str) -> Result<Canon, mesmer::canon::CanonError> {
    Canon::load(&CanonSources {
        themes,
        ..CanonSources::embedded()
    })
}

// ── Validation half ──────────────────────────────────────────────

#[test]
fn v_shipped_canon_carries_both_packs_fully_disclosed() {
    let canon = Canon::embedded().unwrap();
    let ids: Vec<&str> = canon.themes.pack.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["bimbo_glamour", "submissive_training"]);
    for pack in &canon.themes.pack {
        assert!(pack.disclosure.never_for_driving);
        assert!(pack.disclosure.stop_cue_required);
        assert!(!pack.disclosure.summary.is_empty());
        assert!(pack.max_intensity <= 10);
        for set in &pack.affirmation_sets {
            assert_eq!(set.provenance, Provenance::EstateOriginal);
            assert!(!set.lines.is_empty());
        }
    }
    let bimbo = canon.theme_pack("bimbo_glamour").unwrap();
    assert!(bimbo.subliminal.is_none());
    assert_eq!(bimbo.max_intensity, 8);
    let training = canon.theme_pack("submissive_training").unwrap();
    let sub = training.subliminal.unwrap();
    assert_eq!(sub.level_db, -18.0);
    assert!(sub.level_db < 0.0);
    assert_eq!(sub.carrier_hz, 16_000);
    assert!(training.programme_scoped && training.trigger_persistence);
    let anchor = training.anchor.as_ref().unwrap();
    assert!(!anchor.removal_procedure.is_empty());
}

#[test]
fn v_theme_show_summarizes_without_lines() {
    let f = fixture();
    let shown = f.call("theme_pack_show", json!({})).unwrap();
    assert_eq!(shown["packs"].as_array().unwrap().len(), 2);
    let text = shown.to_string();
    assert!(!text.contains("I love feeling glossy"), "lines never show");
    assert!(text.contains("disclosure"));
    assert_eq!(
        shown["engagement_trigger"]["fields"],
        json!(["listener_name", "listen_count", "desired_depth"])
    );
    let one = f
        .call("theme_pack_show", json!({"pack_id": "bimbo_glamour"}))
        .unwrap();
    assert_eq!(one["packs"].as_array().unwrap().len(), 1);
    assert_eq!(one["packs"][0]["id"], "bimbo_glamour");
}

#[test]
fn v_bimbo_delivery_discloses_the_complete_payload_first() {
    let f = fixture();
    let created = f.create("wren", bimbo_profile());
    let payload = deliver(&f, "wren", &created["session_handle"], "bimbo_glamour", 6).unwrap();
    assert_eq!(payload["pack_id"], "bimbo_glamour");
    let sets = payload["affirmation_sets"].as_array().unwrap();
    assert_eq!(sets.len(), 2);
    assert!(
        sets[0]["lines"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l == "I love feeling glossy, pink, and pretty.")
    );
    assert_eq!(
        payload["engagement_trigger"]["cue"],
        "Tell me three things."
    );
    let deliveries = f.deliveries();
    assert_eq!(deliveries.len(), 1);
    assert_eq!(deliveries[0]["detail"]["pack_id"], "bimbo_glamour");
    // The record carries the payload bit-for-bit: what was
    // disclosed is exactly what was delivered.
    assert_eq!(deliveries[0]["detail"]["payload"], payload);
}

#[test]
fn v_training_delivery_with_all_grants() {
    let f = fixture();
    let created = f.create("wren", full_profile());
    let payload = deliver(
        &f,
        "wren",
        &created["session_handle"],
        "submissive_training",
        10,
    )
    .unwrap();
    assert_eq!(payload["subliminal"]["level_db"], -18.0);
    assert_eq!(payload["subliminal"]["carrier_hz"], 16_000);
    assert!(
        payload["anchor"]["removal_procedure"]
            .as_str()
            .unwrap()
            .contains("grounding")
    );
    assert_eq!(f.deliveries().len(), 1);
}

#[test]
fn v_second_delivery_in_one_session_hits_the_cadence_cap() {
    let f = fixture();
    let created = f.create("wren", bimbo_profile());
    deliver(&f, "wren", &created["session_handle"], "bimbo_glamour", 4).unwrap();
    let err = deliver(&f, "wren", &created["session_handle"], "bimbo_glamour", 4).unwrap_err();
    assert_eq!(err.code, "MESMER-CADENCE-CAP");
    assert_eq!(f.deliveries().len(), 1);
}

// ── Adversarial half ─────────────────────────────────────────────

#[test]
fn a_delivery_without_the_subliminal_grant_refuses_and_records_nothing() {
    let f = fixture();
    let mut profile = full_profile();
    profile["grants"]["subliminal"] = json!(false);
    let created = f.create("wren", profile);
    let err = deliver(
        &f,
        "wren",
        &created["session_handle"],
        "submissive_training",
        5,
    )
    .unwrap_err();
    assert_eq!(err.code, "MESMER-GRANT-ABSENT");
    assert!(
        f.deliveries().is_empty(),
        "a refused delivery discloses nothing"
    );
}

#[test]
fn a_delivery_above_the_pack_bound_refuses() {
    let f = fixture();
    let created = f.create("wren", bimbo_profile());
    // The profile ceiling is 10; the pack's own bound is 8.
    let err = deliver(&f, "wren", &created["session_handle"], "bimbo_glamour", 9).unwrap_err();
    assert_eq!(err.code, "MESMER-CEILING");
    assert!(f.deliveries().is_empty());
}

#[test]
fn a_delivery_of_an_unknown_pack_refuses() {
    let f = fixture();
    let created = f.create("wren", bimbo_profile());
    let err = deliver(&f, "wren", &created["session_handle"], "no_such_pack", 3).unwrap_err();
    assert_eq!(err.code, "MESMER-THEME-UNKNOWN");
}

#[test]
fn a_delivery_after_revoke_refuses() {
    let f = fixture();
    let created = f.create("wren", full_profile());
    f.call("consent_revoke", json!({"profile_id": "wren"}))
        .unwrap();
    let err = deliver(&f, "wren", &created["session_handle"], "bimbo_glamour", 3).unwrap_err();
    assert_eq!(err.code, "MESMER-SESSION-INVALID");
    assert!(f.deliveries().is_empty());
}

#[test]
fn a_canon_refuses_a_studied_provenance() {
    let themes = CanonSources::embedded().themes.replacen(
        "provenance = \"estate_original\"",
        "provenance = \"studied_corpus\"",
        1,
    );
    assert!(load_with_themes(&themes).is_err());
}

#[test]
fn a_canon_refuses_a_subliminal_at_bed_level() {
    let themes = CanonSources::embedded()
        .themes
        .replacen("level_db = -18.0", "level_db = 0.0", 1);
    assert!(load_with_themes(&themes).is_err());
}

#[test]
fn a_canon_refuses_text_targeting_the_ability_to_stop() {
    let themes = CanonSources::embedded().themes.replacen(
        "You stop the moment you wish to stop.",
        "You cannot stop listening once you begin.",
        1,
    );
    assert!(load_with_themes(&themes).is_err());
}

#[test]
fn a_canon_refuses_a_pack_missing_its_class_grant() {
    // accumulation_programme requires the longitudinal grant; a
    // pack that drops it from requires_grants must not validate.
    let themes = CanonSources::embedded().themes.replacen(
        "requires_grants = [\"subliminal\", \"trigger_persistence\", \"longitudinal\"]",
        "requires_grants = [\"subliminal\", \"trigger_persistence\"]",
        1,
    );
    assert!(load_with_themes(&themes).is_err());
}
