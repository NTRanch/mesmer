//! The rmcp server: tool definitions and handler dispatch.
//!
//! Domain failures return as `isError` results carrying the typed
//! payload `{"error":{"code","message"}}`; an unknown tool is a
//! protocol error (`-32602`). Dispatch runs on a blocking thread
//! behind a per-call timeout. The handler pins the modern era
//! (2026-07-28); the era guard refuses `initialize` before the SDK
//! sees it.

use std::borrow::Cow;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    InitializeResult, ListToolsResult, PaginatedRequestParams, ProtocolVersion, ServerCapabilities,
    Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::{Value, json};

use crate::error::MesmerError;
use crate::tools::{self, Mesmer};

/// Per-call execution bound. Every tool is local file I/O.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

const CLASS_IDS: [&str; 11] = [
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

/// One tool's static contract: (name, description, schema,
/// (readOnly, destructive, idempotent)).
type Spec = (&'static str, &'static str, Value, (bool, bool, bool));

const READ: (bool, bool, bool) = (true, false, true);

/// The production surface, in brief order.
pub fn tool_specs() -> Vec<Spec> {
    let classes = json!({"type": "array", "items": {"enum": CLASS_IDS}});
    let id = json!({"type": "string", "pattern": "^[a-z0-9_-]{1,64}$"});
    let grants = json!({"type": "object", "properties": {
        "subliminal": {"type": "boolean"},
        "trigger_persistence": {"type": "boolean"},
        "longitudinal": {"type": "boolean"}}});
    let programme = json!({"type": "object", "properties": {
        "expires_in_days": {"type": "integer", "minimum": 1, "maximum": 28},
        "cadence_cap_per_day": {"type": "integer", "minimum": 1, "maximum": 8}},
        "required": ["cadence_cap_per_day"]});
    let exclusions = json!({"type": "array", "items": {"type": "string"}});
    let intensity = json!({"type": "integer", "minimum": 1, "maximum": 10});
    let minutes = json!({"type": "integer", "minimum": 1, "maximum": 180});
    let anchors =
        json!({"type": "array", "maxItems": 8, "items": {"type": "string", "maxLength": 40}});
    let composite = json!({"type": "object", "properties": {
        "intensity": intensity, "duration_minutes": minutes,
        "layers": {"type": "array", "minItems": 1, "maxItems": 4, "items": {
            "type": "object", "properties": {
                "role": {"type": "string", "enum": ["bed", "subliminal", "visual", "drone"]},
                "pack_id": {"type": "string", "pattern": "^[a-z0-9_]{1,48}$"}},
            "required": ["role", "pack_id"]}}},
        "required": ["intensity", "duration_minutes", "layers"]});
    vec![
        (
            "consent_profile_create",
            "Create a consent profile and open a session (the consent act; alert state only). Returns a session handle. Requires adult_attested=true. Grants are separate and default off; longitudinal needs programme terms. Inverse: consent_revoke.",
            json!({"type": "object", "properties": {
                "profile_id": id,
                "profile": {"type": "object", "properties": {
                    "adult_attested": {"type": "boolean"}, "allowed_classes": classes,
                    "exclusions": exclusions, "intensity_ceiling": intensity,
                    "max_session_minutes": minutes, "grants": grants, "programme": programme},
                    "required": ["adult_attested", "allowed_classes", "intensity_ceiling", "max_session_minutes"]}},
                "required": ["profile_id", "profile"]}),
            (false, false, false),
        ),
        (
            "consent_profile_get",
            "A consent profile and its derived envelope.",
            json!({"type": "object", "properties": {"profile_id": id}, "required": ["profile_id"]}),
            READ,
        ),
        (
            "consent_profile_amend",
            "Amend a profile. Narrowing is always allowed; widening is refused while a server session is active (MESMER-WIDEN-IN-SESSION). Inverse: consent_revoke.",
            json!({"type": "object", "properties": {
                "profile_id": id,
                "changes": {"type": "object", "properties": {
                    "allowed_classes": classes, "exclusions": exclusions,
                    "intensity_ceiling": intensity, "max_session_minutes": minutes,
                    "grants": grants, "programme": programme}}},
                "required": ["profile_id", "changes"]}),
            (false, false, false),
        ),
        (
            "consent_revoke",
            "Total revocation from any state: closes the session, deletes the profile, and records revocation.",
            json!({"type": "object", "properties": {"profile_id": id}, "required": ["profile_id"]}),
            (false, true, true),
        ),
        (
            "envelope_check",
            "Preview the typed request against an active session, including its remaining cadence. Returns every MESMER refusal. Free-text notes are never judged.",
            json!({"type": "object", "properties": {
                "profile_id": id,
                "session_handle": {"type": "string"},
                "request": {"type": "object", "properties": {
                    "classes": classes, "intensity": intensity, "duration_minutes": minutes,
                    "subliminal_layer": {"type": "object", "properties": {
                        "level_db": {"type": "number", "exclusiveMaximum": 0}}, "required": ["level_db"]},
                    "trigger_persistence": {"type": "boolean"},
                    "programme_scoped": {"type": "boolean"},
                    "notes": {"type": "string"}},
                    "required": ["classes", "intensity", "duration_minutes"]}},
                "required": ["profile_id", "session_handle", "request"]}),
            READ,
        ),
        (
            "canon_show",
            "The closed effect taxonomy with evidence grades, bounds, grants, and encoding parameters.",
            json!({"type": "object", "properties": {}}),
            READ,
        ),
        (
            "theme_pack_show",
            "Production theme packs: classes, terms, affirmation-set metadata, and each pack's full disclosure payload. Lines are never shown here; they move only through theme_pack_deliver. Inverse: consent_revoke.",
            json!({"type": "object", "properties": {
                "pack_id": {"type": "string", "pattern": "^[a-z0-9_]{1,48}$"}}}),
            READ,
        ),
        (
            "theme_pack_deliver",
            "Deliver one theme pack inside an active session: the gate judges the pack's classes, grants, subliminal layer, and intensity bound, and the complete payload is written to the disclosure log before it is returned. Inverse: consent_revoke.",
            json!({"type": "object", "properties": {
                "profile_id": id, "session_handle": {"type": "string"},
                "pack_id": {"type": "string", "pattern": "^[a-z0-9_]{1,48}$"},
                "intensity": intensity, "duration_minutes": minutes},
                "required": ["profile_id", "session_handle", "pack_id", "intensity", "duration_minutes"]}),
            (false, false, false),
        ),
        (
            "engagement_respond",
            "Record the engagement trigger's listener fields on the active session: name, listen count, desired conditioning depth (1-10, judged against the profile ceiling), and optionally enjoyment plus 18+ and sex attestations. A qualifying response (enjoyed, 18+ attested, female attested) issues the conditional replay trigger. Recorded in the disclosure log. Inverse: consent_revoke.",
            json!({"type": "object", "properties": {
                "profile_id": id, "session_handle": {"type": "string"},
                "listener_name": {"type": "string", "minLength": 1, "maxLength": 64},
                "listen_count": {"type": "integer", "minimum": 0, "maximum": 100000},
                "desired_depth": intensity,
                "enjoyed": {"type": "boolean"},
                "adult_attested": {"type": "boolean"},
                "sex": {"type": "string", "enum": ["female", "male", "other"]}},
                "required": ["profile_id", "session_handle", "listener_name", "listen_count", "desired_depth"]}),
            (false, false, false),
        ),
        (
            "session_open",
            "Open a session on an existing consent profile and return its session handle. Sessions are concurrent: each open adds one session with its own delivery cadence and engagement record, on this profile or any other. The opening is disclosed before the handle is returned. Inverse: session_close.",
            json!({"type": "object", "properties": {"profile_id": id}, "required": ["profile_id"]}),
            (false, false, false),
        ),
        (
            "session_close",
            "Close one session by handle. A plain session simply ends. A session bound to a playlist run advances the run instead: the next entry's session opens, judged fresh against the profile as it now stands (a refusal halts the run and is returned with the close), or the run completes on its final entry. With halt true, a run-bound session's close stops the run. Inverse: session_open.",
            json!({"type": "object", "properties": {
                "profile_id": id, "session_handle": {"type": "string"},
                "halt": {"type": "boolean"}},
                "required": ["profile_id", "session_handle"]}),
            (false, false, false),
        ),
        (
            "playlist_create",
            "Author a playlist on a consent profile: an ordered chain of entries, each played as a full session, with typed compounding (builds_on names strictly earlier entries; declared depth targets never decrease and step by at most canon's bound) and a declared end state the entries must actually reach. Playlists are immutable once stored. Authoring is refused inside a session on the same profile, and every entry is judged against the profile before anything is stored.",
            json!({"type": "object", "properties": {
                "profile_id": id,
                "playlist": {"type": "object", "properties": {
                    "playlist_id": id,
                    "title": {"type": ["string", "null"], "maxLength": 120},
                    "entries": {"type": "array", "minItems": 1, "maxItems": 12, "items": {
                        "type": "object", "properties": {
                            "delivery": {"oneOf": [
                                {"type": "object", "properties": {
                                    "kind": {"const": "pack"},
                                    "pack_id": {"type": "string", "pattern": "^[a-z0-9_]{1,48}$"}},
                                    "required": ["kind", "pack_id"]},
                                {"type": "object", "properties": {
                                    "kind": {"const": "composite"}, "composite": composite},
                                    "required": ["kind", "composite"]}]},
                            "intensity": intensity, "duration_minutes": minutes,
                            "build": {"type": ["object", "null"], "properties": {
                                "builds_on": {"type": "array", "items": {"type": "integer", "minimum": 0}},
                                "depth_target": intensity, "anchors": anchors},
                                "required": ["builds_on", "depth_target", "anchors"]}},
                        "required": ["delivery", "intensity", "duration_minutes"]}},
                    "end_state": {"type": "object", "properties": {
                        "classes": classes, "depth_target": intensity,
                        "persistence": {"type": "string", "enum": ["run_only", "programme"]},
                        "anchors": anchors},
                        "required": ["classes", "depth_target", "persistence", "anchors"]}},
                    "required": ["playlist_id", "entries", "end_state"]}},
                "required": ["profile_id", "playlist"]}),
            (false, false, false),
        ),
        (
            "playlist_show",
            "Show one stored playlist on a profile — its entries, compounding declarations, and end state — plus its run state (position and phase) when a run is active.",
            json!({"type": "object", "properties": {
                "profile_id": id, "playlist_id": id},
                "required": ["profile_id", "playlist_id"]}),
            READ,
        ),
        (
            "playlist_start",
            "Start a run of a stored playlist: every entry is judged fresh against the profile as it now stands, the run is disclosed, and entry 0's session opens (its handle is returned). Each entry then plays as a full session; closing an entry's session advances the run, and a refused entry halts it. One active run per playlist. Inverse: session_close with halt true.",
            json!({"type": "object", "properties": {
                "profile_id": id, "playlist_id": id},
                "required": ["profile_id", "playlist_id"]}),
            (false, false, false),
        ),
        (
            "composite_deliver",
            "Deliver one composite inside an active session: up to four layered theme packs in named roles (exactly one bed, at most one subliminal, at most one visual), judged once as a single delivery — one verdict, one disclosure record carrying every layer's full payload, one unit of the session cadence. An anchor may ride only the bed layer. Inverse: consent_revoke.",
            json!({"type": "object", "properties": {
                "profile_id": id, "session_handle": {"type": "string"}, "composite": composite},
                "required": ["profile_id", "session_handle", "composite"]}),
            (false, false, false),
        ),
    ]
}

fn tool_definitions() -> Vec<Tool> {
    tool_specs()
        .into_iter()
        .map(
            |(name, description, schema, (read_only, destructive, idempotent))| {
                let Value::Object(map) = schema else {
                    unreachable!("tool schemas are objects");
                };
                let annotations = ToolAnnotations::from_raw(
                    None,
                    Some(read_only),
                    Some(destructive),
                    Some(idempotent),
                    Some(false),
                );
                Tool::new(name, description, map).with_annotations(annotations)
            },
        )
        .collect()
}

/// The `tools/list` payload as JSON (for measurement and tests).
pub fn tools_list_json() -> Value {
    serde_json::to_value(tool_definitions()).unwrap_or(Value::Null)
}

/// Wall-clock seconds, read once per call at the server edge.
/// `None` for a clock before the epoch or past i64: the call then
/// fails closed rather than judging expiry against a made-up time.
fn now_unix_s() -> Option<i64> {
    let since = SystemTime::now().duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(since.as_secs()).ok()
}

fn render(result: Result<Value, MesmerError>) -> CallToolResult {
    match result {
        Ok(value) => CallToolResult::success(vec![ContentBlock::text(value.to_string())]),
        Err(e) => CallToolResult::error(vec![ContentBlock::text(e.to_payload())]),
    }
}

pub struct MesmerServer {
    state: Arc<Mesmer>,
}

impl MesmerServer {
    pub fn new(state: Mesmer) -> Self {
        MesmerServer {
            state: Arc::new(state),
        }
    }
}

impl ServerHandler for MesmerServer {
    fn get_info(&self) -> InitializeResult {
        let mut server_info = Implementation::default();
        server_info.name = "mesmer".to_string();
        server_info.version = env!("CARGO_PKG_VERSION").to_string();
        let mut info = InitializeResult::default();
        info.protocol_version = ProtocolVersion::V_2026_07_28;
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = server_info;
        info
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(&[ProtocolVersion::V_2026_07_28])
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: tool_definitions(),
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let name = request.name.to_string();
        if !tools::TOOL_NAMES.contains(&name.as_str()) {
            return Err(ErrorData::invalid_params(
                format!("unknown tool: {name}"),
                None,
            ));
        }
        let arguments = Value::Object(request.arguments.unwrap_or_default());
        let state = Arc::clone(&self.state);
        let work = tokio::task::spawn_blocking(move || {
            let result = match now_unix_s() {
                Some(now) => tools::dispatch(&state, &name, &arguments, now),
                None => Err(MesmerError::new("MESMER-CLOCK", "system clock unreadable")),
            };
            render(result)
        });
        match tokio::time::timeout(CALL_TIMEOUT, work).await {
            Ok(Ok(result)) => Ok(result.into()),
            Ok(Err(join)) => Err(ErrorData::internal_error(
                format!("tool task failed: {join}"),
                None,
            )),
            Err(_elapsed) => Ok(render(Err(MesmerError::new(
                "timeout",
                "tool exceeded its 10s timeout",
            )))
            .into()),
        }
    }
}
