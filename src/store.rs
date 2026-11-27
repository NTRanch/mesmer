//! Persistence: consent profiles as JSON files and the disclosure
//! seam, under one data directory (`MESMER_DATA_DIR`).
//!
//! Layout: `<dir>/profiles/<profile_id>.json` and the append-only
//! `<dir>/disclosure.jsonl`. Profile ids are charset-validated
//! ([`ProfileId`]), so a path built from one cannot leave
//! `profiles/`. Writes are atomic (temp file + fsync + rename);
//! disclosure lines are appended and synced one record at a time.
//! The log records profile lifecycle and pre-delivery disclosures.

use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;
use serde_json::Value;

use crate::canon::Canon;
use crate::consent::{ConsentProfile, PlaylistDefinition, PlaylistId, ProfileId};
use crate::error::{MesmerError, MesmerResult};

/// The env var naming the data directory.
pub const DATA_DIR_ENV: &str = "MESMER_DATA_DIR";
/// Largest profile file the store will read.
const PROFILE_FILE_BYTES_MAX: u64 = 64 * 1024;
/// Largest playlist file the store will read.
const PLAYLIST_FILE_BYTES_MAX: u64 = 64 * 1024;
const DISCLOSURE_FILE: &str = "disclosure.jsonl";
const PROFILES_DIR: &str = "profiles";
const PLAYLISTS_DIR: &str = "playlists";

fn io_error(context: &str, e: &std::io::Error) -> MesmerError {
    MesmerError::new("MESMER-STORE-IO", format!("{context}: {e}"))
}

/// Write `bytes` to `path` atomically (temp file + fsync +
/// rename). The temp name is unique per write (pid + sequence):
/// two concurrent writes of one record must never share a temp
/// file and interleave into a torn rename. A failed write leaves
/// no temp debris behind.
fn write_atomic(path: &Path, bytes: &[u8]) -> MesmerResult<()> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::SeqCst);
    let tmp = path.with_extension(format!("json.tmp-{}-{seq}", std::process::id()));
    let written = (|| -> MesmerResult<()> {
        let mut file = File::create(&tmp).map_err(|e| io_error("cannot create temp", &e))?;
        file.write_all(bytes)
            .map_err(|e| io_error("cannot write temp", &e))?;
        file.sync_all()
            .map_err(|e| io_error("cannot sync temp", &e))?;
        drop(file);
        fs::rename(&tmp, path).map_err(|e| io_error("cannot commit record", &e))
    })();
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written
}

/// Events recorded by the append-only disclosure log.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DisclosureEvent {
    ProfileCreated,
    ProfileAmended,
    ProfileRevoked,
    DeliveryDisclosed,
    EngagementRecorded,
    ReplayTriggerIssued,
    SessionOpened,
    SessionClosed,
    PlaylistCreated,
    PlaylistDeleted,
    PlaylistRunStarted,
    PlaylistEntryOpened,
    PlaylistRunHalted,
    PlaylistRunCompleted,
}

/// One disclosure line.
#[derive(Clone, Debug, Serialize)]
pub struct DisclosureRecord<'a> {
    pub at_unix_s: i64,
    pub event: DisclosureEvent,
    pub profile_id: &'a str,
    pub detail: Value,
}

/// The on-disk store. Owns nothing but its root path.
#[derive(Clone, Debug)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// Open (creating if absent) a store rooted at `root`.
    pub fn open(root: impl Into<PathBuf>) -> MesmerResult<Store> {
        let root = root.into();
        fs::create_dir_all(root.join(PROFILES_DIR))
            .map_err(|e| io_error("cannot create data dir", &e))?;
        Ok(Store { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn disclosure_path(&self) -> PathBuf {
        self.root.join(DISCLOSURE_FILE)
    }

    fn profile_path(&self, id: &ProfileId) -> PathBuf {
        self.root
            .join(PROFILES_DIR)
            .join(format!("{}.json", id.as_str()))
    }

    /// Load a profile. `Ok(None)` when absent; a present but
    /// invalid file is `MESMER-STORE-CORRUPT`, never served.
    pub fn get(&self, id: &ProfileId) -> MesmerResult<Option<ConsentProfile>> {
        let path = self.profile_path(id);
        let file = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io_error("cannot open profile", &e)),
        };
        let mut bytes = Vec::new();
        file.take(PROFILE_FILE_BYTES_MAX + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| io_error("cannot read profile", &e))?;
        if bytes.len() as u64 > PROFILE_FILE_BYTES_MAX {
            return Err(MesmerError::new(
                "MESMER-STORE-CORRUPT",
                "profile file too large",
            ));
        }
        let profile: ConsentProfile = serde_json::from_slice(&bytes)
            .map_err(|e| MesmerError::new("MESMER-STORE-CORRUPT", e.to_string()))?;
        profile
            .validate()
            .map_err(|e| MesmerError::new("MESMER-STORE-CORRUPT", e.message))?;
        if &profile.profile_id != id {
            return Err(MesmerError::new(
                "MESMER-STORE-CORRUPT",
                "profile id mismatch",
            ));
        }
        Ok(Some(profile))
    }

    /// Write a profile atomically (temp + fsync + rename).
    pub fn put(&self, profile: &ConsentProfile) -> MesmerResult<()> {
        let bytes = serde_json::to_vec_pretty(profile)
            .map_err(|e| MesmerError::new("MESMER-STORE-IO", e.to_string()))?;
        write_atomic(&self.profile_path(&profile.profile_id), &bytes)
    }

    /// Delete a profile. Returns whether one existed.
    pub fn delete(&self, id: &ProfileId) -> MesmerResult<bool> {
        match fs::remove_file(self.profile_path(id)) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
            Err(e) => Err(io_error("cannot delete profile", &e)),
        }
    }

    // ── Playlists (1.1.0, playlists SPEC §4.5) ───────────────────

    fn playlist_path(&self, profile: &ProfileId, id: &PlaylistId) -> PathBuf {
        self.root
            .join(PLAYLISTS_DIR)
            .join(profile.as_str())
            .join(format!("{}.json", id.as_str()))
    }

    fn profile_playlists_dir(&self, profile: &ProfileId) -> PathBuf {
        self.root.join(PLAYLISTS_DIR).join(profile.as_str())
    }

    /// Write a playlist atomically, like a profile.
    pub fn put_playlist(&self, playlist: &PlaylistDefinition) -> MesmerResult<()> {
        let dir = self.profile_playlists_dir(&playlist.profile_id);
        fs::create_dir_all(&dir).map_err(|e| io_error("cannot create playlists dir", &e))?;
        let bytes = serde_json::to_vec_pretty(playlist)
            .map_err(|e| MesmerError::new("MESMER-STORE-IO", e.to_string()))?;
        write_atomic(
            &self.playlist_path(&playlist.profile_id, &playlist.playlist_id),
            &bytes,
        )
    }

    /// Load a playlist. `Ok(None)` when absent; a present but
    /// invalid file is `MESMER-STORE-CORRUPT`, never served.
    /// Validation is the full structural validation against
    /// canon (SPEC §2.1): a stored definition that no longer
    /// validates does not exist as a playlist.
    pub fn get_playlist(
        &self,
        profile: &ProfileId,
        id: &PlaylistId,
        canon: &Canon,
    ) -> MesmerResult<Option<PlaylistDefinition>> {
        let path = self.playlist_path(profile, id);
        let file = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io_error("cannot open playlist", &e)),
        };
        let mut bytes = Vec::new();
        file.take(PLAYLIST_FILE_BYTES_MAX + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| io_error("cannot read playlist", &e))?;
        if bytes.len() as u64 > PLAYLIST_FILE_BYTES_MAX {
            return Err(MesmerError::new(
                "MESMER-STORE-CORRUPT",
                "playlist file too large",
            ));
        }
        let playlist: PlaylistDefinition = serde_json::from_slice(&bytes)
            .map_err(|e| MesmerError::new("MESMER-STORE-CORRUPT", e.to_string()))?;
        playlist
            .validate(canon)
            .map_err(|e| MesmerError::new("MESMER-STORE-CORRUPT", e.message))?;
        if &playlist.profile_id != profile || &playlist.playlist_id != id {
            return Err(MesmerError::new(
                "MESMER-STORE-CORRUPT",
                "playlist id mismatch",
            ));
        }
        Ok(Some(playlist))
    }

    /// Delete one playlist. Returns whether one existed.
    pub fn delete_playlist(&self, profile: &ProfileId, id: &PlaylistId) -> MesmerResult<bool> {
        match fs::remove_file(self.playlist_path(profile, id)) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
            Err(e) => Err(io_error("cannot delete playlist", &e)),
        }
    }

    /// The playlist ids stored for one profile (file stems that
    /// parse as ids; anything else in the directory is debris,
    /// not data).
    pub fn list_playlist_ids(&self, profile: &ProfileId) -> MesmerResult<Vec<PlaylistId>> {
        let dir = self.profile_playlists_dir(profile);
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io_error("cannot list playlists", &e)),
        };
        let mut ids = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| io_error("cannot list playlists", &e))?;
            let name = entry.file_name();
            if let Some(stem) = name.to_str().and_then(|n| n.strip_suffix(".json"))
                && let Ok(id) = PlaylistId::try_from(stem.to_string())
            {
                ids.push(id);
            }
        }
        ids.sort();
        Ok(ids)
    }

    /// Delete a profile's whole playlist directory (revocation,
    /// SPEC §4.5). Returns the ids that were removed.
    pub fn delete_profile_playlists(&self, profile: &ProfileId) -> MesmerResult<Vec<PlaylistId>> {
        let ids = self.list_playlist_ids(profile)?;
        if !ids.is_empty() {
            fs::remove_dir_all(self.profile_playlists_dir(profile))
                .map_err(|e| io_error("cannot delete playlists dir", &e))?;
        }
        Ok(ids)
    }

    /// Count one profile's disclosed deliveries in
    /// `[day_start_unix_s, day_end_unix_s)`, read from the
    /// disclosure log (the programme day-cadence input, SPEC
    /// §4.3). Lines that do not parse are not deliveries and are
    /// not counted.
    pub fn deliveries_between(
        &self,
        profile_id: &str,
        day_start_unix_s: i64,
        day_end_unix_s: i64,
    ) -> MesmerResult<u32> {
        let text = match fs::read_to_string(self.disclosure_path()) {
            Ok(text) => text,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(io_error("cannot read disclosure log", &e)),
        };
        let mut count: u32 = 0;
        for line in text.lines() {
            let Ok(record) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let at = record["at_unix_s"].as_i64().unwrap_or(i64::MIN);
            if record["event"] == "delivery_disclosed"
                && record["profile_id"] == profile_id
                && (day_start_unix_s..day_end_unix_s).contains(&at)
            {
                count = count.saturating_add(1);
            }
        }
        Ok(count)
    }

    /// Append one record to the disclosure seam and sync it.
    pub fn append_disclosure(&self, record: &DisclosureRecord<'_>) -> MesmerResult<()> {
        let mut line = serde_json::to_string(record)
            .map_err(|e| MesmerError::new("MESMER-STORE-IO", e.to_string()))?;
        assert!(!line.contains('\n'), "compact JSON is one line");
        line.push('\n');
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.disclosure_path())
            .map_err(|e| io_error("cannot open disclosure log", &e))?;
        file.write_all(line.as_bytes())
            .map_err(|e| io_error("cannot append disclosure", &e))?;
        file.sync_data()
            .map_err(|e| io_error("cannot sync disclosure", &e))
    }
}
