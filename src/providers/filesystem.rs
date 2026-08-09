use super::{Provider, ProviderContext};
use crate::model::{Candidate, Source};
use anyhow::Result;
use std::{fs, path::PathBuf};
pub struct FilesystemProvider;
impl Provider for FilesystemProvider {
    fn name(&self) -> &'static str {
        "filesystem"
    }
    fn matches(&self, c: &ProviderContext<'_>) -> bool {
        matches!(
            c.query.command.as_deref(),
            Some("cd" | "cat" | "code" | "ls" | "open")
        )
    }
    fn complete(&self, c: &ProviderContext<'_>) -> Result<Vec<Candidate>> {
        let raw = &c.query.current;
        let expanded = if raw == "~" || raw.starts_with("~/") {
            dirs::home_dir()
                .map(|h| h.join(raw.trim_start_matches("~/")))
                .unwrap_or_else(|| PathBuf::from(raw))
        } else {
            c.cwd.join(raw)
        };
        let (dir, prefix) = if raw.ends_with('/') {
            (expanded, String::new())
        } else {
            (
                expanded.parent().unwrap_or(c.cwd).to_path_buf(),
                expanded
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            )
        };
        let display_base = raw
            .rsplit_once('/')
            .map(|(b, _)| format!("{b}/"))
            .unwrap_or_default();
        let mut out = Vec::new();
        for e in fs::read_dir(dir)?.flatten().take(1000) {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.starts_with(&prefix) || (name.starts_with('.') && !prefix.starts_with('.')) {
                continue;
            }
            let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
            if c.query.command.as_deref() == Some("cd") && !is_dir {
                continue;
            }
            let mut value = format!("{display_base}{name}");
            if is_dir {
                value.push('/')
            }
            out.push(Candidate::new(
                value,
                if is_dir { "directory" } else { "file" },
                Source::Filesystem,
            ));
        }
        Ok(out)
    }
}
