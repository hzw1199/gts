use std::collections::BTreeMap;
use std::fs;
use std::io;

use serde::{Deserialize, Serialize};

use super::error::StorageError;
use super::home::{index_path, resolve_gts_home, HomeOptions};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceEntry {
    pub path: String,
    pub active: String,
}

pub type IndexData = BTreeMap<String, WorkspaceEntry>;

pub fn read_index(opts: &HomeOptions) -> Result<IndexData, StorageError> {
    let home = resolve_gts_home(opts);
    let file = index_path(&home);
    match fs::read_to_string(&file) {
        Ok(raw) => {
            let parsed: IndexData = serde_json::from_str(&raw).map_err(|err| {
                StorageError::message(format!("Invalid index.json: {} at {}", err, file.display()))
            })?;
            Ok(parsed)
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(IndexData::new()),
        Err(err) => Err(StorageError::io(err)),
    }
}

pub fn write_index(data: &IndexData, opts: &HomeOptions) -> Result<(), StorageError> {
    let home = resolve_gts_home(opts);
    fs::create_dir_all(&home).map_err(StorageError::io)?;
    let body = serde_json::to_string_pretty(data).map_err(|err| {
        StorageError::message(format!("Failed to serialize index.json: {err}"))
    })?;
    // serde_json pretty uses 2-space indent; ensure trailing newline.
    let mut out = body;
    if !out.ends_with('\n') {
        out.push('\n');
    }
    fs::write(index_path(&home), out).map_err(StorageError::io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::home::HomeOptions;

    fn temp_dir(prefix: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn write_index_uses_two_space_indent_and_trailing_newline() {
        let home = temp_dir("gts-index-fmt");
        let opts = HomeOptions::with_home(&home);
        let mut data = IndexData::new();
        data.insert(
            "abc".into(),
            WorkspaceEntry {
                path: "/tmp/wt".into(),
                active: "main".into(),
            },
        );
        write_index(&data, &opts).unwrap();
        let raw = fs::read_to_string(index_path(&home)).unwrap();
        assert!(raw.ends_with('\n'));
        assert!(raw.contains("\n  \"abc\""));
        assert!(!raw.contains("\t"));
    }
}
