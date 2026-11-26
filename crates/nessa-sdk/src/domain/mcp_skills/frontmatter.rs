use super::{FrontmatterValue, McpSkillError, SkillFrontmatter};
use std::collections::BTreeMap;

/// Parse the required YAML frontmatter block at the start of a `SKILL.md` file.
///
/// This is the single parser shared by both server-side [`super::SkillCatalog`]
/// construction and client-side [`super::SkillEntry::verify_file`] frontmatter
/// verification.
pub fn parse_frontmatter(markdown: &str) -> Result<SkillFrontmatter, McpSkillError> {
    let text = markdown.strip_prefix('\u{feff}').unwrap_or(markdown);
    let mut lines = text.split('\n');
    // `split` always yields a first piece; an empty file's is `""`, which is
    // refused below like any other first line that is not `---`.
    let first = lines.next().unwrap_or_default();
    if first.trim_end_matches('\r') != "---" {
        return Err(McpSkillError::InvalidFrontmatter(
            "SKILL.md must start with '---'".to_owned(),
        ));
    }
    let mut yaml_lines = Vec::new();
    let mut closed = false;
    for raw in lines {
        let line = raw.trim_end_matches('\r');
        if line == "---" || line == "..." {
            closed = true;
            break;
        }
        yaml_lines.push(line);
    }
    if !closed {
        return Err(McpSkillError::InvalidFrontmatter(
            "unterminated YAML frontmatter block".to_owned(),
        ));
    }
    let mut index = 0;
    let fields = parse_mapping(&yaml_lines, &mut index, 0)?;
    SkillFrontmatter::new(fields)
}

fn parse_mapping(
    lines: &[&str],
    index: &mut usize,
    base_indent: usize,
) -> Result<BTreeMap<String, FrontmatterValue>, McpSkillError> {
    let mut map = BTreeMap::new();
    while *index < lines.len() {
        let line = lines[*index];
        if is_ignorable(line) {
            *index += 1;
            continue;
        }
        let indent = leading_spaces(line)?;
        if indent < base_indent {
            break;
        }
        if indent > base_indent {
            return Err(McpSkillError::InvalidFrontmatter(format!(
                "unexpected indentation at line {}",
                *index + 2
            )));
        }
        let trimmed = &line[indent..];
        let Some((raw_key, raw_rest)) = split_key_value(trimmed) else {
            return Err(McpSkillError::InvalidFrontmatter(format!(
                "expected 'key: value' at line {}",
                *index + 2
            )));
        };
        let key = parse_key(raw_key)?;
        let rest = raw_rest.trim();
        *index += 1;
        let value = if rest.is_empty() {
            parse_nested_block(lines, index, base_indent)?
        } else if rest == "|" || rest == ">" || rest == "|-" || rest == ">-" {
            parse_block_scalar(lines, index, base_indent, rest)?
        } else {
            parse_inline_value(rest)?
        };
        if map.insert(key.clone(), value).is_some() {
            return Err(McpSkillError::InvalidFrontmatter(format!(
                "duplicate frontmatter key '{key}'"
            )));
        }
    }
    Ok(map)
}

fn parse_nested_block(
    lines: &[&str],
    index: &mut usize,
    parent_indent: usize,
) -> Result<FrontmatterValue, McpSkillError> {
    let mut lookahead = *index;
    while lookahead < lines.len() && is_ignorable(lines[lookahead]) {
        lookahead += 1;
    }
    if lookahead >= lines.len() {
        return Ok(FrontmatterValue::Null);
    }
    let next_indent = leading_spaces(lines[lookahead])?;
    let trimmed = &lines[lookahead][next_indent..];
    let is_seq = trimmed.starts_with("- ") || trimmed == "-";
    if next_indent < parent_indent || (next_indent == parent_indent && !is_seq) {
        return Ok(FrontmatterValue::Null);
    }
    *index = lookahead;
    if is_seq {
        let mut items = Vec::new();
        while *index < lines.len() {
            let line = lines[*index];
            if is_ignorable(line) {
                *index += 1;
                continue;
            }
            let indent = leading_spaces(line)?;
            if indent < next_indent {
                break;
            }
            if indent > next_indent {
                return Err(McpSkillError::InvalidFrontmatter(format!(
                    "unexpected sequence indentation at line {}",
                    *index + 2
                )));
            }
            let item_line = &line[indent..];
            let Some(after_dash) = item_line.strip_prefix('-') else {
                break;
            };
            let rest = after_dash.trim();
            *index += 1;
            if rest.is_empty() {
                items.push(parse_nested_block(lines, index, next_indent)?);
            } else {
                items.push(parse_inline_value(rest)?);
            }
        }
        Ok(FrontmatterValue::Sequence(items))
    } else {
        let nested = parse_mapping(lines, index, next_indent)?;
        Ok(FrontmatterValue::Mapping(nested))
    }
}

fn parse_block_scalar(
    lines: &[&str],
    index: &mut usize,
    parent_indent: usize,
    indicator: &str,
) -> Result<FrontmatterValue, McpSkillError> {
    let mut block_indent: Option<usize> = None;
    let mut collected = Vec::new();
    while *index < lines.len() {
        let line = lines[*index];
        if line.trim().is_empty() {
            collected.push(String::new());
            *index += 1;
            continue;
        }
        let indent = leading_spaces(line)?;
        if indent <= parent_indent {
            break;
        }
        let effective = *block_indent.get_or_insert(indent);
        if indent < effective {
            break;
        }
        collected.push(line[effective..].to_owned());
        *index += 1;
    }
    while collected.last().is_some_and(String::is_empty) {
        collected.pop();
    }
    let folded = indicator.starts_with('>');
    let strip = indicator.ends_with('-');
    let mut out = if folded {
        collected.join(" ")
    } else {
        collected.join("\n")
    };
    if !strip && !out.is_empty() {
        out.push('\n');
    }
    Ok(FrontmatterValue::String(out))
}

/// Parse an inline value, refusing a quoted string or flow sequence whose
/// quotes do not close.
///
/// Only this top-level call can fail: every quote inside a value that passed
/// the check closes too, so [`inline_value`] reads its parts without a check
/// of its own.
fn parse_inline_value(raw: &str) -> Result<FrontmatterValue, McpSkillError> {
    let trimmed = strip_inline_comment(raw).trim();
    let quoted = trimmed.starts_with('"') && trimmed.ends_with('"');
    let flow = trimmed.starts_with('[') && trimmed.ends_with(']');
    if (quoted || flow) && !quotes_close(trimmed) {
        return Err(unterminated_quote());
    }
    Ok(inline_value(trimmed))
}

fn inline_value(trimmed: &str) -> FrontmatterValue {
    if trimmed.is_empty() || trimmed == "null" || trimmed == "~" {
        return FrontmatterValue::Null;
    }
    if trimmed == "true" {
        return FrontmatterValue::Bool(true);
    }
    if trimmed == "false" {
        return FrontmatterValue::Bool(false);
    }
    if let Some(inner) = trimmed.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        return FrontmatterValue::String(unescape_double_quoted(inner));
    }
    if let Some(inner) = trimmed
        .strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''))
    {
        return FrontmatterValue::String(inner.replace("''", "'"));
    }
    if let Some(inner) = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        let inner = inner.trim();
        if inner.is_empty() {
            return FrontmatterValue::Sequence(Vec::new());
        }
        return FrontmatterValue::Sequence(
            split_flow_items(inner)
                .into_iter()
                .map(|part| inline_value(strip_inline_comment(part).trim()))
                .collect(),
        );
    }
    if let Ok(number) = trimmed.parse::<i64>() {
        return FrontmatterValue::Integer(number);
    }
    FrontmatterValue::String(trimmed.to_owned())
}

fn unterminated_quote() -> McpSkillError {
    McpSkillError::InvalidFrontmatter("unterminated quote in frontmatter value".to_owned())
}

/// Whether every quote in `input` closes, reading `\` inside double quotes as
/// an escape the way [`unescape_double_quoted`] does.
fn quotes_close(input: &str) -> bool {
    scan_quotes(input).1
}

/// Split a flow sequence's inside at its top-level commas.
fn split_flow_items(input: &str) -> Vec<&str> {
    scan_quotes(input).0
}

/// One pass over `input`: the pieces between top-level commas, and whether
/// every quote closed.
fn scan_quotes(input: &str) -> (Vec<&str>, bool) {
    let mut items = Vec::new();
    let mut start = 0;
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    for (idx, ch) in input.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_double => escaped = true,
            '"' if !in_single => in_double = !in_double,
            '\'' if !in_double => in_single = !in_single,
            ',' if !in_single && !in_double => {
                items.push(input[start..idx].trim());
                start = idx + 1;
            }
            _ => {}
        }
    }
    items.push(input[start..].trim());
    (items, !in_single && !in_double)
}

/// Unescape the inside of a double-quoted string whose quotes closed, so it
/// never ends in a lone backslash.
fn unescape_double_quoted(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut escaped = false;
    for ch in input.chars() {
        if !escaped {
            if ch == '\\' {
                escaped = true;
            } else {
                out.push(ch);
            }
            continue;
        }
        escaped = false;
        match ch {
            '"' => out.push('"'),
            '\\' => out.push('\\'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            '/' => out.push('/'),
            other => {
                out.push('\\');
                out.push(other);
            }
        }
    }
    out
}

fn strip_inline_comment(raw: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    let bytes = raw.as_bytes();
    for (idx, ch) in raw.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_double => escaped = true,
            '"' if !in_single => in_double = !in_double,
            '\'' if !in_double => in_single = !in_single,
            '#' if !in_single
                && !in_double
                && (idx == 0 || bytes[idx - 1].is_ascii_whitespace()) =>
            {
                return &raw[..idx];
            }
            _ => {}
        }
    }
    raw
}

fn split_key_value(line: &str) -> Option<(&str, &str)> {
    let mut in_single = false;
    let mut in_double = false;
    let bytes = line.as_bytes();
    for (idx, ch) in line.char_indices() {
        match ch {
            '"' if !in_single => in_double = !in_double,
            '\'' if !in_double => in_single = !in_single,
            ':' if !in_single && !in_double => {
                let next_is_sep = idx + 1 == bytes.len() || bytes[idx + 1].is_ascii_whitespace();
                if next_is_sep {
                    return Some((&line[..idx], &line[idx + 1..]));
                }
            }
            _ => {}
        }
    }
    None
}

fn parse_key(raw: &str) -> Result<String, McpSkillError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(McpSkillError::InvalidFrontmatter(
            "empty frontmatter key".to_owned(),
        ));
    }
    if let Some(inner) = trimmed.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
        if !quotes_close(trimmed) {
            return Err(unterminated_quote());
        }
        return Ok(unescape_double_quoted(inner));
    }
    if let Some(inner) = trimmed
        .strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''))
    {
        return Ok(inner.replace("''", "'"));
    }
    Ok(trimmed.to_owned())
}

fn is_ignorable(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.is_empty() || trimmed.starts_with('#')
}

fn leading_spaces(line: &str) -> Result<usize, McpSkillError> {
    let mut count = 0;
    for byte in line.bytes() {
        match byte {
            b' ' => count += 1,
            b'\t' => {
                return Err(McpSkillError::InvalidFrontmatter(
                    "tabs are not permitted for YAML indentation".to_owned(),
                ));
            }
            _ => break,
        }
    }
    Ok(count)
}
