use crate::model::QueryContext;

/// Parses an incomplete shell buffer without evaluating expansions or substitutions.
pub fn parse_context(buffer: &str, cursor: usize) -> QueryContext {
    let cursor = floor_char_boundary(buffer, cursor.min(buffer.len()));
    let before = &buffer[..cursor];
    let raw_current_len = raw_current_len(before);
    let trailing_space = before.chars().last().is_some_and(char::is_whitespace);
    let mut tokens = tokenize_incomplete(before);
    let current = if trailing_space {
        String::new()
    } else {
        tokens.pop().unwrap_or_default()
    };
    let command = tokens.first().cloned().or_else(|| {
        if current.is_empty() {
            None
        } else {
            Some(current.clone())
        }
    });
    let args = if tokens.is_empty() {
        Vec::new()
    } else {
        tokens[1..].to_vec()
    };
    QueryContext {
        buffer: buffer.to_owned(),
        cursor,
        tokens,
        current,
        raw_current_len,
        command,
        args,
    }
}

fn raw_current_len(input: &str) -> usize {
    let mut token_start = 0;
    let mut quote = None;
    let mut escaped = false;
    let mut in_token = false;
    let mut count = 0;
    for ch in input.chars() {
        if escaped {
            escaped = false;
            in_token = true;
        } else if ch == '\\' && quote != Some('\'') {
            escaped = true;
            in_token = true;
        } else if let Some(active) = quote {
            if ch == active {
                quote = None;
            }
            in_token = true;
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
            in_token = true;
        } else if ch.is_whitespace() {
            in_token = false;
            token_start = count + 1;
        } else {
            in_token = true;
        }
        count += 1;
    }
    if in_token { count - token_start } else { 0 }
}

fn floor_char_boundary(value: &str, mut index: usize) -> usize {
    while index > 0 && !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

pub fn tokenize_incomplete(input: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    for ch in input.chars() {
        if escaped {
            word.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else {
                word.push(ch);
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
        } else if ch.is_whitespace() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else {
            word.push(ch);
        }
    }
    if escaped {
        word.push('\\');
    }
    if !word.is_empty() || quote.is_some() {
        words.push(word);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_incomplete_and_quoted_input() {
        let c = parse_context("git clone 'https://github.com/a/r", 35);
        assert_eq!(c.command.as_deref(), Some("git"));
        assert_eq!(c.args, ["clone"]);
        assert_eq!(c.current, "https://github.com/a/r");
    }

    #[test]
    fn trailing_space_creates_empty_current_token() {
        let c = parse_context("npm run ", 8);
        assert_eq!(c.args, ["run"]);
        assert!(c.current.is_empty());
    }

    #[test]
    fn raw_prefix_counts_shell_syntax_for_replacement() {
        assert_eq!(parse_context("cd space\\ d", 11).raw_current_len, 8);
        assert_eq!(parse_context("cd 'space d", 11).raw_current_len, 8);
    }
}
