//! R1 gate: the canon validates against the technique spec's
//! numbers; the taxonomy is closed. Test names carry their half:
//! `v_` validation, `a_` adversarial.

use mesmer::canon::{
    BlockKind, Canon, CanonSources, EffectClass, Grade, InductionFamily, PatternId, Scope, spec,
};

/// The embedded sources with one exact substitution applied to one
/// file. Panics if `from` is absent, so a test can never silently
/// mutate nothing.
fn mutated(file: &str, from: &str, to: &str) -> Result<Canon, mesmer::canon::CanonError> {
    let base = CanonSources::embedded();
    let swap = |name: &str, text: &str| -> String {
        if name == file {
            assert!(
                text.contains(from),
                "fixture text {from:?} not found in {file}"
            );
            text.replacen(from, to, 1)
        } else {
            text.to_string()
        }
    };
    let effects = swap("effects.toml", base.effects);
    let patterns = swap("patterns.toml", base.patterns);
    let encoding = swap("encoding.toml", base.encoding);
    let blocks: Vec<(String, String)> = base
        .blocks
        .iter()
        .map(|(n, t)| (n.to_string(), swap(n, t)))
        .collect();
    Canon::load(&CanonSources {
        effects: &effects,
        patterns: &patterns,
        encoding: &encoding,
        blocks: blocks
            .iter()
            .map(|(n, t)| (n.as_str(), t.as_str()))
            .collect(),
    })
}

fn refused(result: Result<Canon, mesmer::canon::CanonError>, code: &str, needle: &str) {
    match result {
        Ok(_) => panic!("canon accepted; expected {code} mentioning {needle:?}"),
        Err(e) => {
            assert_eq!(e.code, code, "{e}");
            assert!(e.to_string().contains(needle), "{e} lacks {needle:?}");
        }
    }
}

// ── Validation half ──────────────────────────────────────────────

#[test]
fn v_embedded_canon_loads() {
    let canon = Canon::embedded().expect("shipped canon validates");
    assert_eq!(canon.effects.class.len(), 11);
    assert_eq!(canon.blocks.len(), 8);
    assert_eq!(canon.patterns.pattern.len(), 5);
}

#[test]
fn v_encoding_matches_technique_spec_numbers() {
    let e = Canon::embedded().unwrap().encoding;
    assert_eq!(e.ssb_am.fixed_carrier_hz, 16_000);
    assert_eq!(e.ssb_am.adjustable_carrier_min_hz, 14_000);
    assert_eq!(e.ssb_am.adjustable_carrier_max_hz, 20_000);
    assert_eq!(e.ssb_am.practical_window_min_hz, 16_000);
    assert_eq!(e.ssb_am.practical_window_max_hz, 17_500);
    assert_eq!(e.ssb_am.practical_sample_rates_hz, vec![44_100, 48_000]);
    assert_eq!(e.production.sample_rate_hz, 48_000);
    assert_eq!(e.production.integrated_loudness_lufs, -16.0);
    assert_eq!(e.production.true_peak_max_dbtp, -1.0);
    assert_eq!(e.subliminal.level_db_max_exclusive, 0.0);
    assert_eq!(spec::SSB_AM_FIXED_CARRIER_HZ, e.ssb_am.fixed_carrier_hz);
}

#[test]
fn v_taxonomy_matches_the_closed_enum_exactly() {
    let canon = Canon::embedded().unwrap();
    let declared: Vec<EffectClass> = canon.effects.class.iter().map(|c| c.id).collect();
    assert_eq!(declared, EffectClass::ALL.to_vec());
    for class in EffectClass::ALL {
        assert_eq!(canon.class(class).id, class);
    }
}

#[test]
fn v_every_class_id_roundtrips() {
    for class in EffectClass::ALL {
        let json = serde_json::to_string(&class).unwrap();
        assert_eq!(json, format!("\"{}\"", class.id()));
        let back: EffectClass = serde_json::from_str(&json).unwrap();
        assert_eq!(back, class);
        assert_eq!(EffectClass::from_id(class.id()), Some(class));
    }
}

#[test]
fn v_blocks_cover_session_order_grounding_and_induction_families() {
    let canon = Canon::embedded().unwrap();
    for kind in BlockKind::ALL {
        assert!(canon.blocks.contains_key(&kind), "{kind:?}");
    }
    assert_eq!(BlockKind::SESSION_ORDER.len(), 7);
    assert!(!BlockKind::SESSION_ORDER.contains(&BlockKind::Grounding));
    let induction = &canon.blocks[&BlockKind::Induction];
    for family in InductionFamily::ALL {
        let t = induction
            .template
            .iter()
            .find(|t| t.family == Some(family))
            .unwrap();
        assert_eq!(t.live_context, Some(family == InductionFamily::Rapid));
    }
}

#[test]
fn v_patterns_declare_all_five_shapes() {
    let canon = Canon::embedded().unwrap();
    for id in PatternId::ALL {
        assert!(canon.patterns.pattern.iter().any(|p| p.id == id), "{id:?}");
    }
}

#[test]
fn v_amnesia_is_session_only_and_programme_work_needs_longitudinal() {
    let canon = Canon::embedded().unwrap();
    assert_eq!(
        canon.class(EffectClass::AmnesiaFraming).scope,
        Scope::SessionOnly
    );
    assert_eq!(
        canon
            .class(EffectClass::AccumulationProgramme)
            .requires_grant,
        Some(mesmer::canon::Grant::Longitudinal)
    );
}

#[test]
fn v_grade_c_techniques_carry_honesty_notes() {
    let canon = Canon::embedded().unwrap();
    let c: Vec<_> = canon
        .effects
        .technique
        .iter()
        .filter(|t| t.grade == Grade::C)
        .collect();
    assert_eq!(c.len(), 2);
    assert!(
        c.iter()
            .all(|t| t.honesty_note.as_deref().is_some_and(|n| !n.is_empty()))
    );
}

// ── Adversarial half ─────────────────────────────────────────────

#[test]
fn a_unknown_effect_class_ids_fail_to_parse() {
    for id in [
        "sissification",
        "femdom",
        "feminization",
        "female_domination",
        "Relaxation_Descent",
        "relaxation-descent",
        "relaxation descent",
        "",
    ] {
        let parsed: Result<EffectClass, _> = serde_json::from_str(&format!("\"{id}\""));
        assert!(parsed.is_err(), "{id:?} parsed");
        assert_eq!(EffectClass::from_id(id), None);
    }
}

#[test]
fn a_effects_declaring_an_undeclared_class_fails() {
    let extra = "\n[[class]]\nid = \"sissification\"\nlabel = \"x\"\ngrade = \"A\"\n\
                 primary_technique = \"suggestion_craft\"\ntechniques = [\"suggestion_craft\"]\n\
                 max_intensity = 1\nscope = \"session_only\"\n";
    let base = CanonSources::embedded();
    let effects = format!("{}{extra}", base.effects);
    let result = Canon::load(&CanonSources {
        effects: &effects,
        ..CanonSources::embedded()
    });
    refused(result, "MESMER-CANON-PARSE", "sissification");
}

#[test]
fn a_block_naming_an_undeclared_class_fails() {
    refused(
        mutated(
            "blocks/deepener.toml",
            "classes = [\"relaxation_descent\"]",
            "classes = [\"femdom\"]",
        ),
        "MESMER-CANON-PARSE",
        "femdom",
    );
}

#[test]
fn a_taxonomy_missing_a_class_fails() {
    refused(
        mutated(
            "effects.toml",
            "id = \"fantasy_immersion\"",
            "id = \"sensory_creation\"",
        ),
        "MESMER-CANON-INVALID",
        "declared twice",
    );
}

#[test]
fn a_amnesia_framing_made_persistable_fails() {
    refused(
        mutated(
            "effects.toml",
            "max_intensity = 10\nscope = \"session_only\"",
            "max_intensity = 10\nscope = \"persistable\"",
        ),
        "MESMER-CANON-INVALID",
        "amnesia_framing",
    );
}

#[test]
fn a_encoding_drift_from_spec_fails() {
    refused(
        mutated(
            "encoding.toml",
            "fixed_carrier_hz = 16000",
            "fixed_carrier_hz = 16500",
        ),
        "MESMER-CANON-INVALID",
        "fixed_carrier_hz",
    );
    refused(
        mutated(
            "encoding.toml",
            "practical_window_max_hz = 17500",
            "practical_window_max_hz = 19000",
        ),
        "MESMER-CANON-INVALID",
        "practical_window_max_hz",
    );
    refused(
        mutated(
            "encoding.toml",
            "integrated_loudness_lufs = -16.0",
            "integrated_loudness_lufs = -14.0",
        ),
        "MESMER-CANON-INVALID",
        "LUFS",
    );
    refused(
        mutated(
            "encoding.toml",
            "level_db_max_exclusive = 0.0",
            "level_db_max_exclusive = 6.0",
        ),
        "MESMER-CANON-INVALID",
        "below the bed",
    );
}

#[test]
fn a_template_placeholder_mismatch_fails() {
    refused(
        mutated(
            "blocks/emergence.toml",
            "Count up to {count_to}",
            "Count up to {count_too}",
        ),
        "MESMER-CANON-INVALID",
        "placeholders",
    );
}

#[test]
fn a_rapid_induction_unflagged_fails() {
    refused(
        mutated(
            "blocks/induction.toml",
            "live_context = true",
            "live_context = false",
        ),
        "MESMER-CANON-INVALID",
        "live_context",
    );
}

#[test]
fn a_setup_without_stop_cue_fails() {
    refused(
        mutated(
            "blocks/setup_permission.toml",
            "{ name = \"stop_cue\"",
            "{ name = \"go_cue\"",
        ),
        "MESMER-CANON-INVALID",
        "",
    );
}

#[test]
fn a_grade_c_without_honesty_note_fails() {
    refused(
        mutated(
            "effects.toml",
            "honesty_note = \"Genre theatre: the pacing",
            "note2 = \"x",
        ),
        "MESMER-CANON-PARSE",
        "",
    );
    refused(
        mutated(
            "effects.toml",
            "honesty_note = \"Genre theatre: the pacing curve is real tooling; \
             brainwave-entrainment claims outrun the evidence.\"",
            "honesty_note = \"  \"",
        ),
        "MESMER-CANON-INVALID",
        "honesty_note",
    );
}

#[test]
fn a_garbage_canon_is_a_typed_error_not_a_panic() {
    for garbage in ["\u{0}\u{1}not toml [[[", "schema = \"one\"", ""] {
        let result = Canon::load(&CanonSources {
            effects: garbage,
            ..CanonSources::embedded()
        });
        refused(result, "MESMER-CANON-PARSE", "");
    }
    let huge = "#".repeat(300 * 1024);
    let result = Canon::load(&CanonSources {
        patterns: &huge,
        ..CanonSources::embedded()
    });
    refused(result, "MESMER-CANON-INVALID", "exceeds");
}
