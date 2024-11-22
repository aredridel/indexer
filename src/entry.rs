use anyhow::{Error, Result};
use serde::Serialize;
use std::error::Error as StdError;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::UNIX_EPOCH;

#[derive(Serialize, Debug)]
pub struct Entry {
    pub children: Option<Vec<Entry>>,
    pub description: String,
    pub file_name: String,
    pub is_dir: bool,
    pub is_image: bool,
    pub size: u64,
    pub time: u64,
    pub type_marker: String,
}

impl Entry {
    pub fn entries(path: &Path, include_children: bool) -> Result<Vec<Entry>, impl StdError> {
        fs::read_dir(path).map(|r| {
            r.filter_map(|e| {
                e.map_or(None, |de| {
                    if Entry::is_hidden(&de) {
                        None
                    } else {
                        let e = Entry::from_dirent(&de, include_children);
                        match e {
                            Ok(ent) => Some(ent),
                            Err(_) => None,
                        }
                    }
                })
            })
            .collect()
        })
    }

    fn from_dirent(de: &fs::DirEntry, include_children: bool) -> Result<Entry> {
        let is_dir = de.file_type()?.is_dir();
        let metadata = de.metadata()?;
        let xa = xattr::get(de.path(), "description").map_or("".to_string(), |e| {
            e.map_or("".to_string(), |e| String::from_utf8_lossy(&e).to_string())
        });
        Ok(Entry {
            file_name: de
                .file_name()
                .into_string()
                .map_err(|_| Error::msg("non utf-8 path"))?,
            is_image: Entry::is_image(de)?,
            is_dir,
            description: xa,
            time: metadata
                .created()?
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            type_marker: if is_dir { "/" } else { "" }.to_string(),
            size: metadata.size(),
            children: if is_dir && include_children {
                Some(Entry::entries(&de.path(), false)?)
            } else {
                None
            },
        })
    }

    fn is_hidden(entry: &fs::DirEntry) -> bool {
        entry
            .file_name()
            .to_str()
            .map(|s| s.starts_with("."))
            .unwrap_or(true)
    }

    fn is_image(entry: &fs::DirEntry) -> Result<bool> {
        entry
            .file_name()
            .to_str()
            .ok_or_else(|| Error::msg("non utf-8 path"))
            .map(|f| f.ends_with(".png") || f.ends_with(".jpg") || f.ends_with(".jpeg"))
    }
}
