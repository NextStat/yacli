use std::fs;

use chrono::{SecondsFormat, Utc};
use rand::{Rng, distr::Alphanumeric};
use serde::{Deserialize, Serialize};

use crate::activity_undo::ActivityUndoAction;
use crate::error::Result;
use crate::paths::activity_log_path;
use crate::persist::write_config_file;

const ACTIVITY_FILE_VERSION: u32 = 1;
const MAX_ACTIVITY_ENTRIES: usize = 200;

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct ActivityEntry {
    pub id: String,
    pub occurred_at: String,
    pub source: String,
    pub operation: String,
    pub account: String,
    pub summary: String,
    pub replay_command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo: Option<ActivityUndoAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undo_command: Option<String>,
}

#[derive(Clone, Debug)]
pub struct NewActivityEntry {
    pub source: String,
    pub operation: String,
    pub account: String,
    pub summary: String,
    pub replay_command: String,
    pub undo: Option<ActivityUndoAction>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ActivityFile {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    entries: Vec<ActivityEntry>,
}

impl Default for ActivityFile {
    fn default() -> Self {
        Self {
            version: default_version(),
            entries: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ActivityStore {
    file: ActivityFile,
}

impl ActivityStore {
    pub fn load() -> Result<Self> {
        let path = activity_log_path()?;
        if !path.exists() {
            return Ok(Self {
                file: ActivityFile::default(),
            });
        }

        let content = fs::read_to_string(path)?;
        let file = toml::from_str::<ActivityFile>(&content)?;
        Ok(Self { file })
    }

    pub fn save(&self) -> Result<()> {
        let path = activity_log_path()?;
        let content = toml::to_string_pretty(&self.file)?;
        write_config_file(&path, &content)
    }

    pub fn entries(&self) -> &[ActivityEntry] {
        &self.file.entries
    }

    pub fn find(&self, id: &str) -> Option<&ActivityEntry> {
        self.file.entries.iter().find(|entry| entry.id == id)
    }

    pub fn append(&mut self, new_entry: NewActivityEntry) -> ActivityEntry {
        let undo_command = new_entry
            .undo
            .as_ref()
            .map(ActivityUndoAction::command_line);
        let entry = ActivityEntry {
            id: generate_activity_id(),
            occurred_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            source: new_entry.source,
            operation: new_entry.operation,
            account: new_entry.account,
            summary: new_entry.summary,
            replay_command: new_entry.replay_command,
            undo: new_entry.undo,
            undo_command,
        };
        self.file.entries.insert(0, entry.clone());
        if self.file.entries.len() > MAX_ACTIVITY_ENTRIES {
            self.file.entries.truncate(MAX_ACTIVITY_ENTRIES);
        }
        entry
    }
}

pub fn record_activity(new_entry: NewActivityEntry) -> Result<ActivityEntry> {
    let mut store = ActivityStore::load()?;
    let entry = store.append(new_entry);
    store.save()?;
    Ok(entry)
}

const fn default_version() -> u32 {
    ACTIVITY_FILE_VERSION
}

fn generate_activity_id() -> String {
    let suffix: String = rand::rng()
        .sample_iter(Alphanumeric)
        .take(8)
        .map(char::from)
        .collect();
    format!("act_{}_{}", Utc::now().format("%Y%m%dT%H%M%SZ"), suffix)
}
