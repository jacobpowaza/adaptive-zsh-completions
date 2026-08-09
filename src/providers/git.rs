use super::{Provider, ProviderContext};
use crate::{
    model::{Candidate, Source},
    safety::run_informational,
};
use anyhow::Result;
use std::time::Duration;
pub struct GitProvider;
impl Provider for GitProvider {
    fn name(&self) -> &'static str {
        "git"
    }
    fn matches(&self, c: &ProviderContext<'_>) -> bool {
        c.query.command.as_deref() == Some("git")
            && matches!(
                c.query.args.first().map(String::as_str),
                Some("checkout" | "switch" | "push" | "remote")
            )
    }
    fn complete(&self, c: &ProviderContext<'_>) -> Result<Vec<Candidate>> {
        let sub = c.query.args.first().map(String::as_str).unwrap_or("");
        let arg_pos = c.query.args.len();
        let (args, desc) = if matches!(sub, "checkout" | "switch") {
            (
                vec![
                    "for-each-ref",
                    "--format=%(refname:short)",
                    "refs/heads",
                    "refs/remotes",
                ],
                "Git branch",
            )
        } else if sub == "push" && arg_pos <= 1 {
            (vec!["remote"], "Git remote")
        } else if sub == "push" {
            (
                vec!["for-each-ref", "--format=%(refname:short)", "refs/heads"],
                "Git branch",
            )
        } else {
            (vec!["remote"], "Git remote")
        };
        let text = run_informational("git", args, Some(c.cwd), Duration::from_millis(500))?;
        Ok(text
            .lines()
            .filter(|s| !s.contains(" -> "))
            .map(|s| Candidate::new(s.trim(), desc, Source::Dynamic))
            .collect())
    }
}
