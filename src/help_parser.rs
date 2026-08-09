use regex::Regex;

use crate::model::{CommandSchema, ItemKind, SchemaItem};

pub fn parse_help(command: &str, path: &[String], text: &str) -> CommandSchema {
    let option = Regex::new(r"^\s*((?:-[A-Za-z0-9?](?:,?\s+)?)?(?:--[A-Za-z0-9][A-Za-z0-9_-]*)?)(?:[ =]+(?:<([^>]+)>|\[([^\]]+)\]|([A-Z][A-Z0-9_-]*)))?\s{2,}(.*)$").unwrap();
    let subcommand =
        Regex::new(r"^\s{2,}([a-zA-Z0-9][a-zA-Z0-9_-]*(?:,\s*[a-zA-Z0-9_-]+)*)\s{2,}(.+)$")
            .unwrap();
    let usage_re = Regex::new(r"(?i)^\s*usage:\s*(.+)$").unwrap();
    let inline_flag =
        Regex::new(r"(?:^|[\s\[|,])(-[A-Za-z0-9?]|--[A-Za-z0-9][A-Za-z0-9_-]*)").unwrap();
    let mut section = "";
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
        if matches!(heading.as_str(), "options" | "flags" | "global options") {
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
            items.push(SchemaItem {
                names: c[1].split(',').map(|s| s.trim().to_owned()).collect(),
                kind: ItemKind::Subcommand,
                value_hint: None,
                values: vec![],
                description: c[2].trim().to_owned(),
            });
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
    value
        .chars()
        .filter(|c| matches!(c, '\t' | '\n' | ' ') || (!c.is_control() && *c != '\u{1b}'))
        .take(16_384)
        .collect()
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
        assert_eq!(strip_control("safe\u{1b}[31m\nBAD"), "safe[31m\nBAD");
    }
}
