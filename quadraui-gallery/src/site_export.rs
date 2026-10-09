//! Dump the demo registry to JSON for the website generator
//! (`tools/site_gen.py`, quadraui#1349).
//!
//! The mdBook site built from this crate's gallery must have **no
//! hand-maintained per-widget content** (CLAUDE.md's "Demos are
//! mandatory" rule, applied one level up): every widget's code sample
//! and backend-support grid come from this crate's own registry and
//! [`crate::capture::run_capture`]'s `manifest.json`, never copied by
//! hand into a site source file. This module is the registry half of
//! that pair — [`dump_registry`] writes one JSON row per registered
//! [`crate::Demo`] (name, group, variant names, and its
//! `// gallery:begin` / `// gallery:end` source region), which
//! `tools/site_gen.py` then joins against a capture directory's
//! `manifest.json` to build one page per demo.
//!
//! Unlike [`crate::capture::run_capture`], this needs no backend
//! feature at all: [`crate::Demo::name`], [`crate::Demo::group`],
//! [`crate::Demo::variants`] and [`crate::Demo::source`] are all plain
//! data accessors that never touch `&mut dyn Backend`. `src/main.rs`'s
//! `--dump-registry <file>` flag calls this in every backend arm
//! (including the no-backend-features fallback), and the same function
//! a test calls directly — there is no separate "CLI" code path to
//! drift from the one under test, mirroring `run_capture`'s own doc.

use std::fs;
use std::io;
use std::path::Path;

use serde::Serialize;

use crate::registry::registry;

/// One row of `registry.json`: a single registered [`crate::Demo`]'s
/// static shape — name, group, variant names, and its trimmed source
/// region — with no rendering or capture-outcome data (that lives in
/// [`crate::capture::ManifestEntry`] instead).
#[derive(Debug, Clone, Serialize)]
pub struct RegistryEntry {
    pub demo: String,
    pub group: String,
    pub variants: Vec<String>,
    pub source: String,
}

/// Write one [`RegistryEntry`] per [`registry`] entry to `path` as
/// pretty-printed JSON, creating any missing parent directories, and
/// return the entries written.
pub fn dump_registry(path: &Path) -> io::Result<Vec<RegistryEntry>> {
    let entries: Vec<RegistryEntry> = registry()
        .iter()
        .map(|demo| RegistryEntry {
            demo: demo.name().to_string(),
            group: demo.group().to_string(),
            variants: demo.variants().iter().map(|v| v.to_string()).collect(),
            source: demo.source().to_string(),
        })
        .collect();

    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let json = serde_json::to_string_pretty(&entries)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(path, json)?;
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dump_registry_writes_one_row_per_registered_demo_with_a_non_empty_source() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let dest = dir.path().join("registry.json");

        let entries = dump_registry(&dest).expect("dump_registry");
        assert_eq!(entries.len(), registry().len());

        let written = fs::read_to_string(&dest).expect("read registry.json");
        let parsed: serde_json::Value =
            serde_json::from_str(&written).expect("registry.json must be valid JSON");
        let parsed = parsed
            .as_array()
            .expect("registry.json must be a JSON array");
        assert_eq!(parsed.len(), entries.len());
        for entry in &entries {
            assert!(!entry.demo.is_empty(), "every row needs a demo name");
            assert!(
                !entry.variants.is_empty(),
                "every demo has at least one variant"
            );
            assert!(
                !entry.source.trim().is_empty(),
                "{}'s source region must not be empty",
                entry.demo
            );
        }
    }

    #[test]
    fn dump_registry_creates_missing_parent_directories() {
        let dir = tempfile::tempdir().expect("create temp dir");
        let dest = dir
            .path()
            .join("nested")
            .join("deeper")
            .join("registry.json");

        dump_registry(&dest).expect("dump_registry should create parent dirs");
        assert!(dest.exists());
    }
}
