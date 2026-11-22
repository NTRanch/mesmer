//! R2 tools: the consent surface over a temp data dir, plus the
//! wire contract of the real binary. `v_` validation, `a_`
//! adversarial.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use mesmer::canon::Canon;
use mesmer::error::MesmerError;
use mesmer::store::Store;
use mesmer::tools::{Mesmer, dispatch};
use serde_json::{Value, json};

static SEQ: AtomicU64 = AtomicU64::new(0);
const T0: i64 = 1_790_000_000;

/// A fresh state over its own temp dir (removed on drop).
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
    let dir = temp_dir("mesmer-tools");
    let state = Mesmer {
        canon: Canon::embedded().unwrap(),
        store: Store::open(&dir).unwrap(),
    };
    Fixture { state, dir }
}

impl Fixture {
    fn call(&self, name: &str, args: Value) -> Result<Value, MesmerError> {
        dispatch(&self.state, name, &args, T0)
    }

    fn create(&self, id: &str) -> Value {
        self.call(
            "consent_profile_create",
            json!({
                "profile_id": id, "in_session": false,
                "profile": {"adult_attested": true,
                            "allowed_classes": ["relaxation_descent", "fantasy_immersion"],
                            "exclusions": ["degradation"],
                            "intensity_ceiling": 5, "max_session_minutes": 40}
            }),
        )
        .expect("create")
    }

    fn disclosure(&self) -> Vec<Value> {
        let text = std::fs::read_to_string(self.dir.join("disclosure.jsonl")).unwrap_or_default();
        text.lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn profile_bytes(&self, id: &str) -> Option<Vec<u8>> {
        std::fs::read(self.dir.join("profiles").join(format!("{id}.json"))).ok()
    }
}

fn check(f: &Fixture, id: &str, request: Value) -> Value {
    f.call(
        "envelope_check",
        json!({"profile_id": id, "request": request}),
    )
    .unwrap()["decision"]
        .clone()
}

// ── Validation half ──────────────────────────────────────────────

#[test]
fn v_create_get_roundtrip_and_disclosure_record() {
    let f = fixture();
    let created = f.create("alice");
    assert!(
        created["consent_statement"]
            .as_str()
            .unwrap()
            .contains("consenting")
    );
    let got = f
        .call("consent_profile_get", json!({"profile_id": "alice"}))
        .unwrap();
    assert_eq!(got["profile"], created["profile"]);
    assert_eq!(got["envelope"]["intensity_ceiling"], 5);
    let log = f.disclosure();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0]["event"], "profile_created");
    assert_eq!(log[0]["profile_id"], "alice");
}

#[test]
fn v_envelope_check_previews_allow_and_refusals() {
    let f = fixture();
    f.create("alice");
    let ok = check(
        &f,
        "alice",
        json!({"classes": ["fantasy_immersion"], "intensity": 5,
                                       "duration_minutes": 40}),
    );
    assert_eq!(ok, json!({"verdict": "allow"}));
    let no = check(
        &f,
        "alice",
        json!({"classes": ["orgasm_control"], "intensity": 6,
                                       "duration_minutes": 40}),
    );
    assert_eq!(no["verdict"], "refuse");
    let codes: Vec<&str> = no["refusals"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["code"].as_str().unwrap())
        .collect();
    assert_eq!(codes, vec!["MESMER-ENVELOPE-CLASS", "MESMER-CEILING"]);
}

#[test]
fn v_amend_narrow_then_widen_outside_session_records_both() {
    let f = fixture();
    f.create("alice");
    let n = f.call(
        "consent_profile_amend",
        json!({"profile_id": "alice", "in_session": true,
                                                   "changes": {"intensity_ceiling": 2}}),
    );
    assert_eq!(n.unwrap()["change"], "narrowing");
    let w = f.call(
        "consent_profile_amend",
        json!({"profile_id": "alice", "in_session": false,
                                                   "changes": {"intensity_ceiling": 7}}),
    );
    let w = w.unwrap();
    assert_eq!(w["change"], "widening");
    assert_eq!(w["profile"]["revision"], 3);
    let events: Vec<Value> = f.disclosure().iter().map(|r| r["event"].clone()).collect();
    assert_eq!(
        events,
        vec!["profile_created", "profile_amended", "profile_amended"]
    );
}

#[test]
fn v_canon_show_exposes_the_closed_taxonomy() {
    let f = fixture();
    let shown = f.call("canon_show", json!({})).unwrap();
    assert_eq!(shown["closed"], true);
    assert_eq!(shown["classes"].as_array().unwrap().len(), 11);
    assert_eq!(shown["encoding"]["ssb_am"]["fixed_carrier_hz"], 16000);
    let text = shown.to_string();
    assert!(text.contains("Genre theatre"));
}

#[test]
fn v_longitudinal_create_with_terms_and_programme_check() {
    let f = fixture();
    f.call(
        "consent_profile_create",
        json!({
            "profile_id": "bea", "in_session": false,
            "profile": {"adult_attested": true, "allowed_classes": ["accumulation_programme"],
                        "intensity_ceiling": 4, "max_session_minutes": 20,
                        "grants": {"longitudinal": true},
                        "programme": {"expires_in_days": 7, "cadence_cap_per_day": 1}}
        }),
    )
    .unwrap();
    let got = f
        .call("consent_profile_get", json!({"profile_id": "bea"}))
        .unwrap();
    assert_eq!(
        got["profile"]["programme"]["expires_at_unix_s"],
        T0 + 7 * 86_400
    );
    let d = check(
        &f,
        "bea",
        json!({"classes": ["accumulation_programme"], "intensity": 4,
                                    "duration_minutes": 20, "programme_scoped": true}),
    );
    assert_eq!(d["verdict"], "allow");
}

#[test]
fn v_tools_list_is_the_six_tool_surface_with_annotations() {
    let list = mesmer::server::tools_list_json();
    let tools = list.as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, mesmer::tools::TOOL_NAMES.to_vec());
    for t in tools {
        assert!(
            t["annotations"]["readOnlyHint"].is_boolean(),
            "{}",
            t["name"]
        );
        assert_eq!(t["inputSchema"]["type"], "object");
    }
    let revoke = tools
        .iter()
        .find(|t| t["name"] == "consent_revoke")
        .unwrap();
    assert_eq!(revoke["annotations"]["destructiveHint"], true);
}

#[test]
fn v_revoke_then_fresh_consent_in_alert_state() {
    let f = fixture();
    f.create("alice");
    f.call("consent_revoke", json!({"profile_id": "alice"}))
        .unwrap();
    let again = f.create("alice");
    assert_eq!(again["profile"]["revision"], 1);
    let events: Vec<Value> = f.disclosure().iter().map(|r| r["event"].clone()).collect();
    assert_eq!(
        events,
        vec!["profile_created", "profile_revoked", "profile_created"]
    );
}

#[test]
fn v_disclosure_is_one_compact_line_per_event() {
    let f = fixture();
    f.create("alice");
    f.create("bea");
    let text = std::fs::read_to_string(f.dir.join("disclosure.jsonl")).unwrap();
    assert_eq!(text.lines().count(), 2);
    assert!(text.ends_with('\n'));
    for line in text.lines() {
        let record: Value = serde_json::from_str(line).unwrap();
        assert_eq!(record["at_unix_s"], T0);
        assert!(record["detail"]["envelope"].is_object());
    }
}

#[test]
fn v_profile_writes_leave_no_temp_files() {
    let f = fixture();
    f.create("alice");
    f.call(
        "consent_profile_amend",
        json!({"profile_id": "alice", "in_session": false,
                                           "changes": {"max_session_minutes": 30}}),
    )
    .unwrap();
    let names: Vec<String> = std::fs::read_dir(f.dir.join("profiles"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["alice.json"]);
}

// ── Adversarial half ─────────────────────────────────────────────

#[test]
fn a_revoke_during_active_session_allows_and_profile_is_gone() {
    let f = fixture();
    f.create("alice");
    // A session is active: widening refuses...
    let w = f.call(
        "consent_profile_amend",
        json!({"profile_id": "alice", "in_session": true,
                                                   "changes": {"intensity_ceiling": 9}}),
    );
    assert_eq!(w.unwrap_err().code, "MESMER-WIDEN-IN-SESSION");
    // ...but revocation is total and always available.
    let r = f
        .call("consent_revoke", json!({"profile_id": "alice"}))
        .unwrap();
    assert_eq!(r["revoked"], true);
    assert!(f.profile_bytes("alice").is_none());
    let get = f.call("consent_profile_get", json!({"profile_id": "alice"}));
    assert_eq!(get.unwrap_err().code, "MESMER-NO-PROFILE");
    let d = check(
        &f,
        "alice",
        json!({"classes": ["relaxation_descent"], "intensity": 1,
                                      "duration_minutes": 1}),
    );
    assert_eq!(d["refusals"][0]["code"], "MESMER-NO-PROFILE");
    let log = f.disclosure();
    assert_eq!(log.last().unwrap()["event"], "profile_revoked");
    // Idempotent: a second revoke succeeds and records nothing new.
    let again = f
        .call("consent_revoke", json!({"profile_id": "alice"}))
        .unwrap();
    assert_eq!(again["revoked"], false);
    assert_eq!(f.disclosure().len(), log.len());
}

#[test]
fn a_refused_widening_leaves_store_and_log_unchanged() {
    let f = fixture();
    f.create("alice");
    let before = f.profile_bytes("alice").unwrap();
    let log_before = f.disclosure().len();
    let w = f.call(
        "consent_profile_amend",
        json!({"profile_id": "alice", "in_session": true,
        "changes": {"intensity_ceiling": 1, "grants": {"subliminal": true}}}),
    );
    assert_eq!(w.unwrap_err().code, "MESMER-WIDEN-IN-SESSION");
    assert_eq!(f.profile_bytes("alice").unwrap(), before);
    assert_eq!(f.disclosure().len(), log_before);
}

#[test]
fn a_create_refusals_attestation_session_duplicate() {
    let f = fixture();
    let draft = json!({"adult_attested": false, "allowed_classes": [], "intensity_ceiling": 1,
                       "max_session_minutes": 10});
    let e = f.call(
        "consent_profile_create",
        json!({"profile_id": "x", "in_session": false, "profile": draft}),
    );
    assert!(e.unwrap_err().message.contains("adult_attested"));
    let mut ok = draft.clone();
    ok["adult_attested"] = json!(true);
    let e = f.call(
        "consent_profile_create",
        json!({"profile_id": "x", "in_session": true, "profile": ok}),
    );
    assert_eq!(e.unwrap_err().code, "MESMER-WIDEN-IN-SESSION");
    f.create("x");
    let e = f.call(
        "consent_profile_create",
        json!({"profile_id": "x", "in_session": false, "profile": ok}),
    );
    assert_eq!(e.unwrap_err().code, "MESMER-PROFILE-EXISTS");
    assert!(f.profile_bytes("x").is_some());
}

#[test]
fn a_hostile_profile_ids_refuse() {
    let f = fixture();
    for id in [
        "../escape",
        "a/b",
        "",
        "UPPER",
        "x".repeat(65).as_str(),
        "nul\u{0}",
    ] {
        let e = f
            .call("consent_profile_get", json!({"profile_id": id}))
            .unwrap_err();
        assert_eq!(e.code, "invalid_input", "{id:?}");
    }
    assert!(!f.dir.parent().unwrap().join("escape.json").exists());
}

#[test]
fn a_unknown_class_and_smuggled_fields_refuse_typed() {
    let f = fixture();
    f.create("alice");
    for request in [
        json!({"classes": ["sissification"], "intensity": 1, "duration_minutes": 5}),
        json!({"classes": ["relaxation_descent"], "intensity": 1, "duration_minutes": 5,
               "override_ceiling": true}),
        json!({"classes": ["relaxation_descent"], "intensity": 1, "duration_minutes": 0}),
    ] {
        let e = f.call(
            "envelope_check",
            json!({"profile_id": "alice", "request": request}),
        );
        assert_eq!(e.unwrap_err().code, "invalid_input", "{request}");
    }
    let e = f.call(
        "consent_profile_amend",
        json!({"profile_id": "alice", "in_session": false,
        "changes": {"allowed_classes": ["femdom"]}}),
    );
    assert_eq!(e.unwrap_err().code, "invalid_input");
}

#[test]
fn a_grant_terms_must_pair_and_stay_bounded() {
    let f = fixture();
    let base = json!({"adult_attested": true, "allowed_classes": ["accumulation_programme"],
                      "intensity_ceiling": 3, "max_session_minutes": 20});
    let mut no_terms = base.clone();
    no_terms["grants"] = json!({"longitudinal": true});
    let mut too_long = no_terms.clone();
    too_long["programme"] = json!({"expires_in_days": 29, "cadence_cap_per_day": 1});
    let mut orphan_terms = base.clone();
    orphan_terms["programme"] = json!({"cadence_cap_per_day": 1});
    for (i, draft) in [no_terms, too_long, orphan_terms].into_iter().enumerate() {
        let e = f.call(
            "consent_profile_create",
            json!({"profile_id": format!("p{i}"), "in_session": false, "profile": draft}),
        );
        assert_eq!(e.unwrap_err().code, "invalid_input", "case {i}");
        assert!(f.profile_bytes(&format!("p{i}")).is_none());
    }
}

#[test]
fn a_corrupt_profile_file_is_refused_never_served() {
    let f = fixture();
    f.create("alice");
    let path = f.dir.join("profiles/alice.json");
    let mut tampered: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    tampered["adult_attested"] = json!(false);
    std::fs::write(&path, tampered.to_string()).unwrap();
    let e = f
        .call("consent_profile_get", json!({"profile_id": "alice"}))
        .unwrap_err();
    assert_eq!(e.code, "MESMER-STORE-CORRUPT");
    let e = f.call(
        "envelope_check",
        json!({"profile_id": "alice", "request":
        {"classes": ["relaxation_descent"], "intensity": 1, "duration_minutes": 1}}),
    );
    assert_eq!(e.unwrap_err().code, "MESMER-STORE-CORRUPT");
    // Revocation still works on a corrupt profile.
    assert_eq!(
        f.call("consent_revoke", json!({"profile_id": "alice"}))
            .unwrap()["revoked"],
        true
    );
}

// ── Wire contract (the real binary) ──────────────────────────────

struct Session {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    dir: PathBuf,
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn spawn(dir: &Path) -> Session {
    let mut child = Command::new(env!("CARGO_BIN_EXE_mesmer"))
        .env("MESMER_DATA_DIR", dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn mesmer");
    let stdin = child.stdin.take().unwrap();
    let stdout = BufReader::new(child.stdout.take().unwrap());
    Session {
        child,
        stdin,
        stdout,
        dir: dir.to_path_buf(),
    }
}

impl Session {
    fn send(&mut self, frame: &Value) -> Value {
        writeln!(self.stdin, "{frame}").unwrap();
        self.stdin.flush().unwrap();
        let mut buf = String::new();
        self.stdout.read_line(&mut buf).unwrap();
        serde_json::from_str(buf.trim()).unwrap_or_else(|_| panic!("non-JSON: {buf}"))
    }

    fn request(&mut self, id: u64, method: &str, mut params: Value) -> Value {
        params.as_object_mut().unwrap().insert(
            "_meta".into(),
            json!({
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientInfo": {"name": "mesmer-contract", "version": "0"},
                "io.modelcontextprotocol/clientCapabilities": {}
            }),
        );
        self.send(&json!({"id": id, "jsonrpc": "2.0", "method": method, "params": params}))
    }
}

#[test]
fn v_wire_discover_list_and_call() {
    let mut s = spawn(&temp_dir("mesmer-wire"));
    let d = s.request(1, "server/discover", json!({}));
    assert_eq!(
        d["result"]["supportedVersions"],
        json!(["2026-07-28"]),
        "{d}"
    );
    let l = s.request(2, "tools/list", json!({}));
    assert_eq!(l["result"]["tools"].as_array().unwrap().len(), 6, "{l}");
    let c = s.request(
        3,
        "tools/call",
        json!({"name": "canon_show", "arguments": {}}),
    );
    assert_eq!(c["result"]["isError"], false, "{c}");
}

#[test]
fn a_wire_legacy_initialize_and_unknown_tool_refuse() {
    let mut s = spawn(&temp_dir("mesmer-wire"));
    let r = s.send(&json!({"id": 1, "jsonrpc": "2.0", "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "legacy", "version": "0"}}}));
    assert_eq!(r["error"]["code"], -32022, "{r}");
    let u = s.request(
        2,
        "tools/call",
        json!({"name": "session_compose", "arguments": {}}),
    );
    assert_eq!(u["error"]["code"], -32602, "{u}");
    let e = s.request(
        3,
        "tools/call",
        json!({"name": "consent_profile_get",
                                              "arguments": {"profile_id": "nobody"}}),
    );
    assert_eq!(e["result"]["isError"], true, "{e}");
    let text = e["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("MESMER-NO-PROFILE"), "{text}");
}

#[test]
fn a_startup_refuses_without_data_dir() {
    let out = Command::new(env!("CARGO_BIN_EXE_mesmer"))
        .env_remove("MESMER_DATA_DIR")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("MESMER_DATA_DIR"));
}
