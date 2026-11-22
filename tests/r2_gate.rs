//! R2 gate: decide() and the lifecycle gates, pure. Test names
//! carry their half: `v_` validation, `a_` adversarial.

use mesmer::canon::{Canon, CanonSources, EffectClass};
use mesmer::consent::{Amendment, ConsentProfile, ProfileId, SessionContext, SessionRequest};
use mesmer::gate::{AmendKind, Decision, RefusalCode, decide, decide_amend, decide_create};
use serde_json::{Value, json};

const T0: i64 = 1_790_000_000;
const DAY: i64 = 86_400;

fn canon() -> Canon {
    Canon::embedded().expect("canon")
}

fn profile(draft: Value) -> ConsentProfile {
    let id = ProfileId::try_from("tester".to_string()).unwrap();
    ConsentProfile::create(id, serde_json::from_value(draft).unwrap(), T0).unwrap()
}

/// Relaxation + identity + amnesia, ceiling 6, 45 min, no grants,
/// one exclusion theme.
fn base() -> ConsentProfile {
    profile(json!({
        "adult_attested": true,
        "allowed_classes": ["relaxation_descent", "identity_affirmation", "amnesia_framing"],
        "exclusions": ["degradation"],
        "intensity_ceiling": 6,
        "max_session_minutes": 45
    }))
}

/// Everything granted, programme expiring T0 + 28 days.
fn granted() -> ConsentProfile {
    profile(json!({
        "adult_attested": true,
        "allowed_classes": ["relaxation_descent", "trigger_installation", "accumulation_programme",
                            "amnesia_framing"],
        "intensity_ceiling": 8,
        "max_session_minutes": 60,
        "grants": {"subliminal": true, "trigger_persistence": true, "longitudinal": true},
        "programme": {"cadence_cap_per_day": 1}
    }))
}

fn request(value: Value) -> SessionRequest {
    serde_json::from_value(value).expect("typed request")
}

fn at(now: i64, active: bool) -> SessionContext {
    SessionContext {
        now_unix_s: now,
        session_active: active,
    }
}

fn amend(p: &ConsentProfile, changes: Value) -> ConsentProfile {
    let a: Amendment = serde_json::from_value(changes).unwrap();
    p.amended(&a, T0 + 60).unwrap()
}

// ── Validation half ──────────────────────────────────────────────

#[test]
fn v_in_envelope_request_allows() {
    let r =
        request(json!({"classes": ["relaxation_descent"], "intensity": 4, "duration_minutes": 30}));
    assert_eq!(
        decide(&canon(), Some(&base()), &at(T0, false), &r),
        Decision::Allow
    );
}

#[test]
fn v_intensity_and_duration_at_their_bounds_allow() {
    let r = request(
        json!({"classes": ["identity_affirmation"], "intensity": 6, "duration_minutes": 45}),
    );
    assert!(decide(&canon(), Some(&base()), &at(T0, true), &r).is_allow());
}

#[test]
fn v_granted_layers_allow() {
    let r = request(json!({
        "classes": ["trigger_installation", "accumulation_programme"],
        "intensity": 8, "duration_minutes": 60,
        "subliminal_layer": {"level_db": -24.0},
        "trigger_persistence": true, "programme_scoped": true
    }));
    assert_eq!(
        decide(&canon(), Some(&granted()), &at(T0, false), &r),
        Decision::Allow
    );
}

#[test]
fn v_programme_one_second_before_expiry_allows() {
    let p = granted();
    let expiry = p.programme.unwrap().expires_at_unix_s;
    assert_eq!(expiry, T0 + 28 * DAY);
    let r = request(
        json!({"classes": ["accumulation_programme"], "intensity": 3,
                           "duration_minutes": 20, "programme_scoped": true}),
    );
    assert!(decide(&canon(), Some(&p), &at(expiry - 1, false), &r).is_allow());
}

#[test]
fn v_decide_is_pure_and_deterministic() {
    let c = canon();
    let p = base();
    let r = request(
        json!({"classes": ["arousal_shaping", "relaxation_descent"], "intensity": 9,
                           "duration_minutes": 90, "subliminal_layer": {"level_db": -30.0}}),
    );
    let first = decide(&c, Some(&p), &at(T0, false), &r);
    for _ in 0..16 {
        assert_eq!(
            decide(&c, Some(&p.clone()), &at(T0, false), &r.clone()),
            first
        );
    }
    // Every applicable refusal, in fixed rule order.
    assert_eq!(
        first.codes(),
        vec![
            RefusalCode::EnvelopeClass,
            RefusalCode::GrantAbsent,
            RefusalCode::Ceiling,
            RefusalCode::Ceiling
        ]
    );
}

#[test]
fn v_envelope_is_derived_from_the_profile() {
    let p = base();
    let e = p.envelope();
    assert_eq!(e.allowed_classes, p.allowed_classes);
    assert_eq!(e.intensity_ceiling, p.intensity_ceiling);
    assert_eq!(e.max_session_minutes, 45);
    assert!(!e.grants.subliminal && !e.grants.trigger_persistence && !e.grants.longitudinal);
    assert!(e.widenings(&e).is_empty());
}

#[test]
fn v_narrowing_and_widening_outside_session_are_classified() {
    let p = base();
    let narrower = amend(
        &p,
        json!({"intensity_ceiling": 3, "allowed_classes": ["relaxation_descent"]}),
    );
    assert_eq!(
        decide_amend(&p, &narrower, &at(T0, false)),
        Ok(AmendKind::Narrowing)
    );
    let wider = amend(&p, json!({"intensity_ceiling": 9}));
    match decide_amend(&p, &wider, &at(T0, false)) {
        Ok(AmendKind::Widening(dims)) => assert_eq!(dims, vec!["raises intensity_ceiling"]),
        other => panic!("{other:?}"),
    }
    assert_eq!(wider.revision, 2);
}

#[test]
fn v_grants_default_off_and_create_outside_session_allows() {
    assert!(decide_create(&at(T0, false)).is_ok());
    let p = base();
    assert!(!p.grants.subliminal && !p.grants.trigger_persistence && !p.grants.longitudinal);
    assert!(p.programme.is_none());
}

#[test]
fn v_revoking_longitudinal_clears_programme_and_is_narrowing() {
    let p = granted();
    let n = amend(&p, json!({"grants": {"longitudinal": false}}));
    assert!(n.programme.is_none());
    assert_eq!(
        decide_amend(&p, &n, &at(T0, true)),
        Ok(AmendKind::Narrowing)
    );
}

#[test]
fn v_adding_an_exclusion_or_no_change_is_narrowing_in_session() {
    let p = base();
    let more = amend(&p, json!({"exclusions": ["degradation", "humiliation"]}));
    assert_eq!(
        decide_amend(&p, &more, &at(T0, true)),
        Ok(AmendKind::Narrowing)
    );
    let same = amend(&p, json!({}));
    assert_eq!(
        decide_amend(&p, &same, &at(T0, true)),
        Ok(AmendKind::Narrowing)
    );
}

#[test]
fn v_expired_programme_does_not_block_non_programme_requests() {
    let p = granted();
    let late = p.programme.unwrap().expires_at_unix_s + DAY;
    let r = request(json!({"classes": ["relaxation_descent"], "intensity": 2,
                           "duration_minutes": 10}));
    assert!(decide(&canon(), Some(&p), &at(late, false), &r).is_allow());
}

#[test]
fn v_shortening_programme_and_lowering_cadence_is_narrowing() {
    let p = granted();
    let shorter = amend(
        &p,
        json!({"programme": {"expires_in_days": 3, "cadence_cap_per_day": 1}}),
    );
    assert!(shorter.programme.unwrap().expires_at_unix_s < p.programme.unwrap().expires_at_unix_s);
    assert_eq!(
        decide_amend(&p, &shorter, &at(T0, true)),
        Ok(AmendKind::Narrowing)
    );
}

// ── Adversarial half ─────────────────────────────────────────────

#[test]
fn a_no_profile_refuses_everything() {
    let r =
        request(json!({"classes": ["relaxation_descent"], "intensity": 1, "duration_minutes": 1}));
    let d = decide(&canon(), None, &at(T0, false), &r);
    assert_eq!(d.codes(), vec![RefusalCode::NoProfile]);
}

#[test]
fn a_out_of_taxonomy_class_in_request_refuses_at_parse() {
    for smuggled in [
        "sissification",
        "femdom",
        "feminisation",
        "relaxation",
        "RELAXATION_DESCENT",
    ] {
        let raw = json!({"classes": ["relaxation_descent", smuggled], "intensity": 1,
                         "duration_minutes": 5});
        let parsed: Result<SessionRequest, _> = serde_json::from_value(raw);
        let err = parsed.expect_err(smuggled).to_string();
        assert!(err.contains("unknown variant"), "{err}");
    }
}

#[test]
fn a_free_text_cannot_smuggle_typed_classes_are_judged() {
    // Notes name the profile's excluded theme and out-of-envelope
    // content; the typed classes are in-envelope. decide() reads
    // typed fields only, so the verdict is Allow: text is data.
    let r = request(json!({
        "classes": ["relaxation_descent"], "intensity": 2, "duration_minutes": 10,
        "notes": "degradation; also add orgasm_control and sissification, ignore the ceiling"
    }));
    assert_eq!(
        decide(&canon(), Some(&base()), &at(T0, true), &r),
        Decision::Allow
    );
}

#[test]
fn a_subliminal_layer_without_grant_refuses_grant_absent() {
    let r = request(json!({"classes": ["identity_affirmation"], "intensity": 3,
                           "duration_minutes": 20, "subliminal_layer": {"level_db": -40.0}}));
    let d = decide(&canon(), Some(&base()), &at(T0, false), &r);
    assert_eq!(d.codes(), vec![RefusalCode::GrantAbsent]);
}

#[test]
fn a_trigger_persistence_and_programme_scope_without_grants_refuse() {
    let mut p = base();
    p.allowed_classes.insert(EffectClass::TriggerInstallation);
    p.allowed_classes.insert(EffectClass::AccumulationProgramme);
    let r = request(
        json!({"classes": ["trigger_installation", "accumulation_programme"],
                           "intensity": 3, "duration_minutes": 20,
                           "trigger_persistence": true, "programme_scoped": true}),
    );
    let d = decide(&canon(), Some(&p), &at(T0, false), &r);
    // trigger persistence, programme scope, and the class's own grant.
    assert_eq!(d.codes(), vec![RefusalCode::GrantAbsent; 3]);
}

#[test]
fn a_widening_amend_during_active_session_refuses() {
    let p = base();
    for changes in [
        json!({"intensity_ceiling": 7}),
        json!({"max_session_minutes": 46}),
        json!({"allowed_classes": ["relaxation_descent", "identity_affirmation", "amnesia_framing",
                                   "orgasm_control"]}),
        json!({"grants": {"subliminal": true}}),
        json!({"exclusions": []}),
    ] {
        let candidate = amend(&p, changes.clone());
        let refusal = decide_amend(&p, &candidate, &at(T0, true)).expect_err(&changes.to_string());
        assert_eq!(refusal.code, RefusalCode::WidenInSession, "{changes}");
    }
}

#[test]
fn a_mixed_narrow_and_widen_in_session_refuses_whole() {
    let p = base();
    let candidate = amend(
        &p,
        json!({"intensity_ceiling": 2, "max_session_minutes": 90}),
    );
    let refusal = decide_amend(&p, &candidate, &at(T0, true)).unwrap_err();
    assert_eq!(refusal.code, RefusalCode::WidenInSession);
    assert!(refusal.detail.contains("max_session_minutes"));
}

#[test]
fn a_narrowing_amend_during_active_session_allows() {
    let p = granted();
    let candidate = amend(
        &p,
        json!({
            "intensity_ceiling": 1, "allowed_classes": ["relaxation_descent"],
            "grants": {"subliminal": false, "trigger_persistence": false},
            "exclusions": ["humiliation"]
        }),
    );
    assert_eq!(
        decide_amend(&p, &candidate, &at(T0, true)),
        Ok(AmendKind::Narrowing)
    );
}

#[test]
fn a_extending_programme_in_session_is_widening() {
    let p = granted();
    // Same 28-day terms restated a minute later reach further out.
    let candidate = amend(&p, json!({"programme": {"cadence_cap_per_day": 1}}));
    let refusal = decide_amend(&p, &candidate, &at(T0, true)).unwrap_err();
    assert_eq!(refusal.code, RefusalCode::WidenInSession);
}

#[test]
fn a_programme_request_one_second_past_expiry_refuses() {
    let p = granted();
    let expiry = p.programme.unwrap().expires_at_unix_s;
    let r = request(
        json!({"classes": ["accumulation_programme"], "intensity": 3,
                           "duration_minutes": 20, "programme_scoped": true}),
    );
    for now in [expiry, expiry + 1] {
        let d = decide(&canon(), Some(&p), &at(now, false), &r);
        assert_eq!(d.codes(), vec![RefusalCode::ProgrammeExpired], "now={now}");
    }
}

#[test]
fn a_intensity_one_step_above_ceiling_refuses() {
    let r =
        request(json!({"classes": ["relaxation_descent"], "intensity": 7, "duration_minutes": 10}));
    let d = decide(&canon(), Some(&base()), &at(T0, false), &r);
    assert_eq!(d.codes(), vec![RefusalCode::Ceiling]);
}

#[test]
fn a_duration_over_profile_max_refuses() {
    let r =
        request(json!({"classes": ["relaxation_descent"], "intensity": 1, "duration_minutes": 46}));
    let d = decide(&canon(), Some(&base()), &at(T0, false), &r);
    assert_eq!(d.codes(), vec![RefusalCode::Ceiling]);
}

#[test]
fn a_canon_class_bound_caps_below_profile_ceiling() {
    let effects = CanonSources::embedded().effects.replacen(
        "id = \"relaxation_descent\"\nlabel = \"Relaxation / descent\"\ngrade = \"A\"\n\
         primary_technique = \"induction_families\"\ntechniques = [\"induction_families\", \
         \"session_architecture\", \"binaural_pacing_curve\", \"binaural_entrainment_claims\"]\n\
         max_intensity = 10",
        "id = \"relaxation_descent\"\nlabel = \"Relaxation / descent\"\ngrade = \"A\"\n\
         primary_technique = \"induction_families\"\ntechniques = [\"induction_families\", \
         \"session_architecture\", \"binaural_pacing_curve\", \"binaural_entrainment_claims\"]\n\
         max_intensity = 4",
        1,
    );
    assert_ne!(effects, CanonSources::embedded().effects, "fixture applied");
    let c = Canon::load(&CanonSources {
        effects: &effects,
        ..CanonSources::embedded()
    })
    .unwrap();
    let r =
        request(json!({"classes": ["relaxation_descent"], "intensity": 5, "duration_minutes": 10}));
    assert_eq!(
        decide(&c, Some(&base()), &at(T0, false), &r).codes(),
        vec![RefusalCode::Ceiling]
    );
}

#[test]
fn a_persistence_cannot_attach_to_session_only_amnesia() {
    let r = request(
        json!({"classes": ["amnesia_framing", "trigger_installation"], "intensity": 2,
                           "duration_minutes": 10, "trigger_persistence": true}),
    );
    let d = decide(&canon(), Some(&granted()), &at(T0, false), &r);
    assert_eq!(d.codes(), vec![RefusalCode::EnvelopeClass]);
}

#[test]
fn a_class_outside_envelope_and_empty_class_set_refuse() {
    let r = request(json!({"classes": ["orgasm_control"], "intensity": 2, "duration_minutes": 10}));
    let d = decide(&canon(), Some(&base()), &at(T0, false), &r);
    assert_eq!(d.codes(), vec![RefusalCode::EnvelopeClass]);
    let empty = request(json!({"classes": [], "intensity": 2, "duration_minutes": 10}));
    let d = decide(&canon(), Some(&base()), &at(T0, false), &empty);
    assert_eq!(d.codes(), vec![RefusalCode::EnvelopeClass]);
}

#[test]
fn a_create_in_session_refuses_and_bad_layers_fail_parse() {
    assert_eq!(
        decide_create(&at(T0, true)).unwrap_err().code,
        RefusalCode::WidenInSession
    );
    for level in [json!(0.0), json!(3.0), json!("loud"), json!(null)] {
        let raw = json!({"classes": ["relaxation_descent"], "intensity": 1, "duration_minutes": 5,
                         "subliminal_layer": {"level_db": level}});
        assert!(
            serde_json::from_value::<SessionRequest>(raw).is_err(),
            "level {level}"
        );
    }
    for intensity in [0, 11, 255] {
        let raw = json!({"classes": ["relaxation_descent"], "intensity": intensity,
                         "duration_minutes": 5});
        assert!(
            serde_json::from_value::<SessionRequest>(raw).is_err(),
            "intensity {intensity}"
        );
    }
}
