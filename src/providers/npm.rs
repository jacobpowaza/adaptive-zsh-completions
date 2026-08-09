use super::{Provider, ProviderContext};
use crate::model::{Candidate, Source};
use anyhow::Result;
use serde_json::Value;
use std::fs;
pub struct NpmProvider;
impl Provider for NpmProvider {
    fn name(&self) -> &'static str {
        "npm"
    }
    fn matches(&self, c: &ProviderContext<'_>) -> bool {
        matches!(c.query.command.as_deref(), Some("npm" | "pnpm" | "yarn"))
            && c.query.args.first().is_some_and(|s| s == "run")
    }
    fn complete(&self, c: &ProviderContext<'_>) -> Result<Vec<Candidate>> {
        let mut dir = Some(c.cwd);
        while let Some(d) = dir {
            let p = d.join("package.json");
            if p.is_file() {
                let v: Value = serde_json::from_slice(&fs::read(p)?)?;
                let mut out = Vec::new();
                if let Some(s) = v.get("scripts").and_then(Value::as_object) {
                    for (name, cmd) in s {
                        out.push(Candidate::new(
                            name,
                            cmd.as_str().unwrap_or("npm script"),
                            Source::Dynamic,
                        ));
                    }
                }
                if let Some(ws) = v.get("workspaces").and_then(Value::as_array) {
                    for w in ws.iter().filter_map(Value::as_str) {
                        out.push(Candidate::new(w, "workspace", Source::Dynamic));
                    }
                }
                return Ok(out);
            }
            dir = d.parent()
        }
        Ok(Vec::new())
    }
}
