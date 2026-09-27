//! Reserved upstream defaults, persisted only after outstanding journals recover.
use super::*;

// src-tauri/src/utils/tmpl.rs, pinned upstream revision documented in UPSTREAM.md.
pub const DEFAULT_GLOBAL_MERGE: &str =
    "# Profile Enhancement Merge Template for Clash Verge\n\nprofile:\n  store-selected: true\n";
pub const DEFAULT_GLOBAL_SCRIPT: &str =
    "// Define main function (script entry)\n\nfunction main(config, profileName) {\n  return config;\n}\n";

impl ProfileStore {
    /// Idempotent initialization. Publish both rows in one catalog rename; never
    /// overwrite source files or initialize against an unrecovered catalog.
    pub fn ensure_global_defaults(&mut self) -> Result<()> {
        for journal in ["profile-refresh.yaml", "profile-merge.yaml", "profile-delete.yaml"] {
            ensure!(
                !self.data_dir.join(journal).try_exists()?,
                "profile recovery is pending"
            );
        }
        // Upstream preserves an existing catalog without a loaded items list.
        if self.profiles.items.is_none() && self.data_dir.join("profiles.yaml").try_exists()? {
            return Ok(());
        }
        let mut candidate = self.profiles.clone();
        let mut additions = Vec::new();
        for (uid, kind, extension, source) in [
            ("Merge", "merge", "yaml", DEFAULT_GLOBAL_MERGE),
            ("Script", "script", "js", DEFAULT_GLOBAL_SCRIPT),
        ] {
            if self.get_item(uid).is_ok() {
                continue;
            }
            // Unique filenames also tolerate files left by an interrupted initialization.
            let file = format!("{uid}-{}.{extension}", unique_id()?);
            candidate.items.get_or_insert_default().push(PrfItem {
                uid: Some(uid.into()),
                itype: Some(kind.into()),
                file: Some(file.clone().into()),
                updated: Some(usize::try_from(
                    SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
                )?),
                ..Default::default()
            });
            additions.push((uid, file, source));
        }
        if additions.is_empty() {
            return Ok(());
        }
        let result = (|| {
            for (_, file, source) in &additions {
                write_new(&self.data_dir.join("profiles").join(file), source.as_bytes())?;
            }
            sync_directory(&self.data_dir.join("profiles"))?;
            self.save(candidate)
        })();
        if result.is_err() {
            for (uid, file, _) in additions {
                if self.get_item(uid).ok().and_then(|item| item.file.as_deref()) != Some(file.as_str()) {
                    let _ = fs::remove_file(self.data_dir.join("profiles").join(file));
                }
            }
        }
        result
    }
}
