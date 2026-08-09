mod docker;
mod filesystem;
mod git;
mod github;
mod npm;
mod ssh;

use crate::{
    cache::Cache,
    model::{Candidate, QueryContext},
};
use anyhow::Result;
use std::path::Path;

pub struct ProviderContext<'a> {
    pub query: &'a QueryContext,
    pub cwd: &'a Path,
    pub cache: &'a Cache,
    pub github_enabled: bool,
    pub docker_enabled: bool,
    pub offline: bool,
}
pub trait Provider {
    fn name(&self) -> &'static str;
    fn matches(&self, ctx: &ProviderContext<'_>) -> bool;
    fn complete(&self, ctx: &ProviderContext<'_>) -> Result<Vec<Candidate>>;
}

pub fn candidates(ctx: &ProviderContext<'_>) -> Vec<Candidate> {
    let providers: Vec<Box<dyn Provider>> = vec![
        Box::new(github::GithubProvider),
        Box::new(git::GitProvider),
        Box::new(npm::NpmProvider),
        Box::new(ssh::SshProvider),
        Box::new(docker::DockerProvider),
        Box::new(filesystem::FilesystemProvider),
    ];
    let mut out = Vec::new();
    for provider in providers {
        if provider.matches(ctx) {
            if let Ok(mut values) = provider.complete(ctx) {
                out.append(&mut values)
            }
        }
    }
    out
}

pub use github::{GithubInput, parse_github_input};
