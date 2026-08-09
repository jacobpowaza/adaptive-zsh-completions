use super::{Provider, ProviderContext};
use crate::{
    model::{Candidate, Source},
    safety::run_successful_informational,
};
use anyhow::Result;
use std::time::Duration;
pub struct DockerProvider;
impl Provider for DockerProvider {
    fn name(&self) -> &'static str {
        "docker"
    }
    fn matches(&self, c: &ProviderContext<'_>) -> bool {
        c.docker_enabled
            && c.query.command.as_deref() == Some("docker")
            && matches!(
                c.query.args.first().map(String::as_str),
                Some("exec" | "logs")
            )
    }
    fn complete(&self, c: &ProviderContext<'_>) -> Result<Vec<Candidate>> {
        let all = c.query.args.first().is_some_and(|s| s == "logs");
        let mut args = vec!["ps"];
        if all {
            args.push("-a")
        }
        args.extend(["--format", "{{.Names}}"]);
        let text = run_successful_informational("docker", args, None, Duration::from_millis(600))?;
        Ok(text
            .lines()
            .map(|v| Candidate::new(v, "Docker container", Source::Dynamic))
            .collect())
    }
}
