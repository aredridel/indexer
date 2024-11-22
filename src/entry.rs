#[derive(Serialize, Debug)]
pub struct Entry {
    children: Option<Vec<Entry>>,
    description: String,
    file_name: String,
    is_dir: bool,
    is_image: bool,
    size: u64,
    time: u64,
    type_marker: String,
}

impl Entry {
    fn entries(path: &Path, include_children: bool) -> Result<Vec<Entry>, impl StdError> {
        fs::read_dir(path).map(|r| {
            r.filter_map(|e| {
                e.map_or(None, |de| {
                    if self.is_hidden(&de) {
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

    fn from_dirent(de: &fs::DirEntry, include_children: bool) -> Result<Entry, Error> {
        let is_dir = de.file_type()?.is_dir();
        let metadata = de.metadata()?;
        let xa = xattr::get(de.path(), "description").map_or("".to_string(), |e| {
            e.map_or("".to_string(), |e| String::from_utf8_lossy(&e).to_string())
        });
        Ok(Entry {
            file_name: de
                .file_name()
                .into_string()
                .map_err(|_e| Error::msg("non-utf-8 filename"))?,
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

    fn is_image(entry: &fs::DirEntry) -> Result<bool, Error> {
        entry
            .file_name()
            .to_str()
            .ok_or_else(|| Error::msg("non utf-8 path"))
            .map(|f| f.ends_with(".png") || f.ends_with(".jpg") || f.ends_with(".jpeg"))
    }
}
