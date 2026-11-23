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

use crate::consent::{ConsentProfile, ProfileId};
use crate::error::{MesmerError, MesmerResult};

/// The env var naming the data directory.
pub const DATA_DIR_ENV: &str = "MESMER_DATA_DIR";
/// Largest profile file the store will read.
const PROFILE_FILE_BYTES_MAX: u64 = 64 * 1024;
const DISCLOSURE_FILE: &str = "disclosure.jsonl";
const PROFILES_DIR: &str = "profiles";

fn io_error(context: &str, e: &std::io::Error) -> MesmerError {
    MesmerError::new("MESMER-STORE-IO", format!("{context}: {e}"))
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

    /// Write a profile atomically (temp + fsync + rename). The
    /// temp name is unique per write (pid + sequence): two
    /// concurrent writes of one profile must never share a temp
    /// file and interleave into a torn rename.
    pub fn put(&self, profile: &ConsentProfile) -> MesmerResult<()> {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let path = self.profile_path(&profile.profile_id);
        let seq = SEQ.fetch_add(1, Ordering::SeqCst);
        let tmp = path.with_extension(format!("json.tmp-{}-{seq}", std::process::id()));
        let bytes = serde_json::to_vec_pretty(profile)
            .map_err(|e| MesmerError::new("MESMER-STORE-IO", e.to_string()))?;
        let written = (|| -> MesmerResult<()> {
            let mut file = File::create(&tmp).map_err(|e| io_error("cannot create temp", &e))?;
            file.write_all(&bytes)
                .map_err(|e| io_error("cannot write temp", &e))?;
            file.sync_all()
                .map_err(|e| io_error("cannot sync temp", &e))?;
            drop(file);
            fs::rename(&tmp, &path).map_err(|e| io_error("cannot commit profile", &e))
        })();
        if written.is_err() {
            // A failed write leaves no temp debris behind.
            let _ = fs::remove_file(&tmp);
        }
        written
    }

    /// Delete a profile. Returns whether one existed.
    pub fn delete(&self, id: &ProfileId) -> MesmerResult<bool> {
        match fs::remove_file(self.profile_path(id)) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
            Err(e) => Err(io_error("cannot delete profile", &e)),
        }
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
