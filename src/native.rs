use std::{path::Path, time::Duration};

use crate::{
    model::{Candidate, CommandSchema, ItemKind, Source},
    safety::{run_informational, sanitize_remote},
};

/// Boundary for safe, machine-oriented completion protocols.
pub trait NativeCompletionAdapter {
    fn detects(&self, root: &CommandSchema) -> bool;
    fn complete(
        &self,
        executable: &str,
        args: &[String],
        cwd: &Path,
    ) -> anyhow::Result<Vec<Candidate>>;
}

/// Cobra CLIs expose a hidden `__complete` endpoint terminated by a directive.
pub struct CobraAdapter;

impl NativeCompletionAdapter for CobraAdapter {
    fn detects(&self, root: &CommandSchema) -> bool {
        root.items
            .iter()
            .filter(|item| item.kind == ItemKind::Subcommand)
            .flat_map(|item| &item.names)
            .any(|name| {
                matches!(
                    name.replace(['-', '_'], "").as_str(),
                    "completion" | "completions" | "shellcompletion"
                )
            })
            && root
                .usage
                .as_deref()
                .is_some_and(|usage| usage.contains("[flags]") || usage.contains("[command]"))
    }

    fn complete(
        &self,
        executable: &str,
        args: &[String],
        cwd: &Path,
    ) -> anyhow::Result<Vec<Candidate>> {
        let mut native = vec!["__complete".to_owned()];
        native.extend(args.iter().cloned());
        let output = run_informational(executable, native, Some(cwd), Duration::from_millis(600))?;
        Ok(output
            .lines()
            .filter(|line| !line.starts_with(':'))
            .filter_map(|line| {
                let (value, description) =
                    line.split_once('\t').unwrap_or((line, "native completion"));
                Some(Candidate::new(
                    sanitize_remote(value)?,
                    sanitize_remote(description).unwrap_or_default(),
                    Source::Native,
                ))
            })
            .collect())
    }
}

pub fn native_candidates(
    root: &CommandSchema,
    executable: &str,
    args: &[String],
    cwd: &Path,
) -> Vec<Candidate> {
    let adapters: [&dyn NativeCompletionAdapter; 1] = [&CobraAdapter];
    adapters
        .into_iter()
        .find(|adapter| adapter.detects(root))
        .and_then(|adapter| adapter.complete(executable, args, cwd).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SchemaItem;

    #[test]
    fn detects_completion_semantically() {
        let schema = CommandSchema {
            usage: Some("tool [command] [flags]".into()),
            items: vec![SchemaItem {
                names: vec!["shell-completion".into()],
                kind: ItemKind::Subcommand,
                value_hint: None,
                values: vec![],
                description: String::new(),
            }],
            ..Default::default()
        };
        assert!(CobraAdapter.detects(&schema));
    }
}
