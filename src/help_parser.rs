use regex::Regex;

use crate::model::{CommandSchema, ItemKind, SchemaItem};

fn add_subcommand_if_new(items: &mut Vec<SchemaItem>, name: &str) {
    if items
        .iter()
        .any(|item| item.kind == ItemKind::Subcommand && item.names.iter().any(|n| n == name))
    {
        return;
    }
    items.push(SchemaItem {
        names: vec![name.to_owned()],
        kind: ItemKind::Subcommand,
        value_hint: None,
        values: vec![],
        description: String::new(),
    });
}

pub fn parse_help(command: &str, path: &[String], text: &str) -> CommandSchema {
    let option = Regex::new(r"^\s*((?:-[A-Za-z0-9?](?:,?\s+)?)?(?:--[A-Za-z0-9][A-Za-z0-9_-]*)?)(?:[ =]+(?:<([^>]+)>|\[([^\]]+)\]|([A-Z][A-Z0-9_-]*)))?\s{2,}(.*)$").unwrap();
    let subcommand =
        Regex::new(r"^\s{2,}([a-zA-Z0-9][a-zA-Z0-9_-]*(?:,\s*[a-zA-Z0-9_-]+)*):?\s{2,}(.+)$")
            .unwrap();
    // A wrapped bullet list ("- access", "- upgrade-interactive / upgradeInteractive").
    let bullet = Regex::new(r"^-\s+([a-zA-Z][a-zA-Z0-9_-]*)").unwrap();
    let usage_re = Regex::new(r"(?i)^\s*usage:\s*(.+)$").unwrap();
    let inline_flag =
        Regex::new(r"(?:^|[\s\[|,])(-[A-Za-z0-9?]|--[A-Za-z0-9][A-Za-z0-9_-]*)").unwrap();
    // Some CLIs (brew, npm, ...) never print a "Commands:" table at all --
    // their examples/usage lines just repeat the invocation, e.g.
    // "brew install FORMULA|CASK...". Recognize that shape directly: the
    // line starts with this exact command (plus any already-known path, for
    // nested subcommand help) followed by a lowercase word, which is almost
    // always the subcommand name itself.
    let mut invocation = command.to_owned();
    for segment in path {
        invocation.push(' ');
        invocation.push_str(segment);
    }
    let usage_line = Regex::new(&format!(
        r"^\s*{}\s+([a-z][a-z0-9_-]*)",
        regex::escape(&invocation)
    ))
    .unwrap();
    // Others (npm's "All commands:") list bare names wrapped across several
    // comma-separated lines with no per-item description at all.
    let comma_list =
        Regex::new(r"^[a-zA-Z][a-zA-Z0-9_-]*(?:,\s*[a-zA-Z][a-zA-Z0-9_-]*)*,?$").unwrap();
    let mut section = "";
    // False inside prose sections (examples, help): invocation-shaped lines
    // there describe usage, not available subcommands.
    let mut extractable = true;
    let mut items = Vec::new();
    let mut usage = None;

    for raw in text.lines().take(4000) {
        let line = strip_control(raw);
        let trimmed = line.trim();
        if let Some(c) = usage_re.captures(trimmed) {
            usage = Some(c[1].trim().to_owned());
        }
        let heading = trimmed.trim_end_matches(':').to_ascii_lowercase();
        if matches!(
            heading.as_str(),
            "commands" | "subcommands" | "available commands"
        ) || (heading.contains("commands") && heading.len() < 100)
        {
            section = "commands";
            continue;
        }
        if matches!(heading.as_str(), "options" | "flags" | "global options")
            || ((heading.contains("options") || heading.contains("flags")) && heading.len() < 100)
        {
            section = "options";
            continue;
        }
        if matches!(
            heading.as_str(),
            "arguments" | "args" | "positional arguments"
        ) {
            section = "arguments";
            continue;
        }
        // Many CLIs group their command table under thematic headings instead
        // of a literal "Commands:" ("Manage your dependencies:", "CORE
        // COMMANDS", ...). Treat any other heading-shaped line as one more
        // such group, unless it's a doc-convention section that is never a
        // command list -- otherwise "ENVIRONMENT VARIABLES" or "EXAMPLES"
        // would have their columnar entries misread as subcommands.
        const NON_COMMAND_HEADINGS: &[&str] = &[
            "example",
            "examples",
            "description",
            "notes",
            "note",
            "see also",
            "environment",
            "environment variables",
            "files",
            "author",
            "authors",
            "bugs",
            "copyright",
            "exit status",
            "exit codes",
            "homepage",
            "reporting bugs",
            "license",
            "aliases",
            "configuration",
            "help",
        ];
        if !trimmed.is_empty() && trimmed.ends_with(':') && trimmed.len() < 100 {
            section = if NON_COMMAND_HEADINGS.contains(&heading.as_str()) {
                ""
            } else {
                "commands"
            };
            // Prose sections (examples, help text) mention invocations like
            // `poetry list` that must never become completion candidates.
            extractable = !NON_COMMAND_HEADINGS.contains(&heading.as_str());
            continue;
        }
        if !trimmed.is_empty()
            && trimmed.len() < 60
            && trimmed == trimmed.to_ascii_uppercase()
            && trimmed.chars().any(char::is_alphabetic)
            && !trimmed.starts_with('-')
        {
            // All-caps section headers with no trailing colon (man-page /
            // `gh`-style: "CORE COMMANDS", "EXAMPLES").
            section = if NON_COMMAND_HEADINGS.contains(&heading.as_str()) {
                ""
            } else {
                "commands"
            };
            extractable = !NON_COMMAND_HEADINGS.contains(&heading.as_str());
            continue;
        }

        if (section == "options" || trimmed.starts_with('-'))
            && let Some(c) = option.captures(&line)
        {
            let names: Vec<_> = c[1]
                .split(',')
                .flat_map(str::split_whitespace)
                .filter(|s| s.starts_with('-'))
                .map(str::to_owned)
                .collect();
            if !names.is_empty() {
                let hint = c
                    .get(2)
                    .or_else(|| c.get(3))
                    .or_else(|| c.get(4))
                    .map(|m| m.as_str().to_owned());
                let values = hint
                    .as_deref()
                    .filter(|h| h.contains('|'))
                    .map(|h| h.split('|').map(|v| v.trim().to_owned()).collect())
                    .unwrap_or_default();
                items.push(SchemaItem {
                    names,
                    kind: ItemKind::Flag,
                    value_hint: hint,
                    values,
                    description: c.get(5).map_or("", |m| m.as_str()).trim().to_owned(),
                });
                continue;
            }
        }
        if section == "commands"
            && let Some(c) = subcommand.captures(&line)
        {
            let names: Vec<String> = c[1].split(',').map(|s| s.trim().to_owned()).collect();
            let description = c[2].trim().to_owned();
            if let Some(existing) = items.iter_mut().find(|item| {
                item.kind == ItemKind::Subcommand && item.names.iter().any(|n| names.contains(n))
            }) {
                existing.names = names;
                if existing.description.is_empty() {
                    existing.description = description;
                }
            } else {
                items.push(SchemaItem {
                    names,
                    kind: ItemKind::Subcommand,
                    value_hint: None,
                    values: vec![],
                    description,
                });
            }
        } else if section == "arguments"
            && let Some(c) = subcommand.captures(&line)
        {
            items.push(SchemaItem {
                names: vec![c[1].trim().to_owned()],
                kind: ItemKind::Positional,
                value_hint: Some(c[1].trim().to_owned()),
                values: vec![],
                description: c[2].trim().to_owned(),
            });
        } else if section == "commands"
            && extractable
            && trimmed.contains(',')
            && comma_list.is_match(trimmed)
        {
            // A wrapped, description-less list of names (npm's "All
            // commands:" block): every comma-separated word is a subcommand.
            for name in trimmed.split(',') {
                let name = name.trim();
                if !name.is_empty() {
                    add_subcommand_if_new(&mut items, name);
                }
            }
        } else if section == "commands"
            && extractable
            && let Some(c) = bullet.captures(trimmed)
        {
            // "- name" / "- name / alias" bullet lists (yarn's "Commands:").
            add_subcommand_if_new(&mut items, &c[1]);
        }
        if extractable && let Some(c) = usage_line.captures(&line) {
            add_subcommand_if_new(&mut items, &c[1]);
        }
        for capture in inline_flag.captures_iter(&line) {
            let name = capture[1].to_owned();
            if !items.iter().any(|item| item.names.contains(&name)) {
                items.push(SchemaItem {
                    names: vec![name],
                    kind: ItemKind::Flag,
                    value_hint: None,
                    values: vec![],
                    description: String::new(),
                });
            }
        }
    }
    items.sort_by(|a, b| a.names.cmp(&b.names));
    items.dedup_by(|a, b| a.names == b.names && a.kind == b.kind);
    let confidence = if items.is_empty() {
        0.2
    } else if usage.is_some() {
        0.88
    } else {
        0.72
    };
    CommandSchema {
        command: command.to_owned(),
        path: path.to_vec(),
        usage,
        items,
        confidence,
        ..Default::default()
    }
}

pub fn strip_control(value: &str) -> String {
    let mut output = String::new();
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{8}' {
            output.pop();
        } else if ch == '\u{1b}' {
            if chars.next_if_eq(&'[').is_some() {
                for control in chars.by_ref() {
                    if ('@'..='~').contains(&control) {
                        break;
                    }
                }
            } else {
                let _ = chars.next();
            }
        } else if matches!(ch, '\t' | '\n' | ' ') || !ch.is_control() {
            output.push(ch);
            if output.len() >= 16_384 {
                break;
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_help_shapes() {
        let schema = parse_help(
            "tool",
            &[],
            "Usage: tool [OPTIONS] <COMMAND>\n\nCommands:\n  build       Build it\n  serve, s    Serve it\n\nOptions:\n  -v, --verbose        Verbose\n      --format <json|text>  Output format\n",
        );
        assert_eq!(
            schema
                .items
                .iter()
                .filter(|i| i.kind == ItemKind::Subcommand)
                .count(),
            2
        );
        let format = schema
            .items
            .iter()
            .find(|i| i.names.contains(&"--format".into()))
            .unwrap();
        assert_eq!(format.values, ["json", "text"]);
    }

    #[test]
    fn strips_terminal_escape_characters() {
        assert_eq!(strip_control("safe\u{1b}[31m\nB\u{8}BAD"), "safe\nBAD");
    }

    #[test]
    fn parses_real_kubectl_grouped_command_sections() {
        // Captured from kubectl v1.36: command tables live under grouped
        // headings ("Basic Commands (Beginner):", "Deploy Commands:", ...)
        // instead of one literal "Commands:" table.
        let schema = parse_help(
            "kubectl",
            &[],
            include_str!("../tests/fixtures/kubectl_help.txt"),
        );
        let names: Vec<&str> = schema
            .items
            .iter()
            .filter(|i| i.kind == ItemKind::Subcommand)
            .flat_map(|i| i.names.iter().map(String::as_str))
            .collect();
        for expected in [
            "get",
            "apply",
            "rollout",
            "config",
            "port-forward",
            "api-resources",
            "describe",
            "logs",
            "scale",
            "annotate",
            "completion",
        ] {
            assert!(names.contains(&expected), "missing {expected} in {names:?}");
        }
        assert!(!names.iter().any(|n| n.starts_with('-')));
    }

    #[test]
    fn parses_real_kubectl_nested_help() {
        let rollout = parse_help(
            "kubectl",
            &["rollout".into()],
            include_str!("../tests/fixtures/kubectl_rollout_help.txt"),
        );
        let names: Vec<&str> = rollout
            .items
            .iter()
            .filter(|i| i.kind == ItemKind::Subcommand)
            .flat_map(|i| i.names.iter().map(String::as_str))
            .collect();
        assert_eq!(
            names,
            ["history", "pause", "restart", "resume", "status", "undo"]
        );

        let config = parse_help(
            "kubectl",
            &["config".into()],
            include_str!("../tests/fixtures/kubectl_config_help.txt"),
        );
        let names: Vec<&str> = config
            .items
            .iter()
            .filter(|i| i.kind == ItemKind::Subcommand)
            .flat_map(|i| i.names.iter().map(String::as_str))
            .collect();
        assert!(names.contains(&"use-context"));
        assert!(names.contains(&"current-context"));
        assert!(names.contains(&"set-context"));
    }

    #[test]
    fn symfony_bare_help_has_no_command_table() {
        // Poetry (Symfony Console): `--help` describes the hidden `list`
        // command, which is why discovery must probe `list` itself.
        let schema = parse_help(
            "poetry",
            &[],
            include_str!("../tests/fixtures/poetry_help.txt"),
        );
        assert_eq!(
            schema
                .items
                .iter()
                .filter(|i| i.kind == ItemKind::Subcommand)
                .count(),
            0
        );
    }

    #[test]
    fn parses_symfony_list_command_table() {
        let schema = parse_help(
            "poetry",
            &[],
            include_str!("../tests/fixtures/poetry_list.txt"),
        );
        let names: Vec<&str> = schema
            .items
            .iter()
            .filter(|i| i.kind == ItemKind::Subcommand)
            .flat_map(|i| i.names.iter().map(String::as_str))
            .collect();
        for expected in ["install", "add", "build", "lock", "publish"] {
            assert!(names.contains(&expected), "missing {expected} in {names:?}");
        }
    }
}
