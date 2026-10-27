use super::SemanticTokenTypeKind;
use emmylua_code_analysis::CuaDocument;
use lsp_types::SemanticToken;
use std::ops::Range;

/// Reorder projected tokens in source order and use UTF-16 source columns.
/// Synthetic annotation keywords and parameter names must never reach the editor.
pub(super) fn map_tokens(document: &CuaDocument, tokens: Vec<SemanticToken>) -> Vec<SemanticToken> {
    let Ok(projection) = &document.projection else {
        return vec![];
    };
    let generated_lines = lines(&projection.analysis_lua);
    let original_lines = lines(&document.source);
    let mut absolute = Vec::new();
    let mut add = |range: Range<usize>, kind: u32, modifiers: u32| {
        let line = original_lines
            .partition_point(|(start, _)| *start <= range.start)
            .saturating_sub(1);
        let (line_start, text) = original_lines[line];
        let content_end = line_start + text.trim_end_matches(['\r', '\n']).len();
        let end = range.end.min(content_end);
        if range.start >= end {
            return;
        }
        let Some(prefix) = document.source.get(line_start..range.start) else {
            return;
        };
        let Some(text) = document.source.get(range.start..end) else {
            return;
        };
        absolute.push((
            line as u32,
            prefix.encode_utf16().count() as u32,
            text.encode_utf16().count() as u32,
            kind,
            modifiers,
        ));
    };
    let mut line = 0u32;
    let mut column = 0u32;
    for token in tokens {
        line += token.delta_line;
        column = if token.delta_line == 0 {
            column + token.delta_start
        } else {
            token.delta_start
        };
        let Some(&(start, text)) = generated_lines.get(line as usize) else {
            continue;
        };
        let byte_at = |col: u32| {
            text.char_indices()
                .nth(col as usize)
                .map_or(text.len(), |(offset, _)| offset)
        };
        let from = start + byte_at(column);
        let to = start + byte_at(column.saturating_add(token.length));
        if let Some(range) = projection.original_token_range(from..to) {
            add(range, token.token_type, token.token_modifiers_bitset);
        }
    }
    for range in &projection.type_markers {
        let kind = if matches!(&document.source[range.clone()], "as" | "end") {
            SemanticTokenTypeKind::Keyword
        } else {
            SemanticTokenTypeKind::Operator
        };
        add(range.clone(), kind.to_u32(), 0);
    }
    absolute.sort_unstable();
    let mut result = Vec::new();
    let mut previous_line = 0;
    let mut previous_column = 0;
    let mut previous_end = 0;
    for (line, column, length, token_type, token_modifiers_bitset) in absolute {
        if line == previous_line && column < previous_end {
            continue;
        }
        result.push(SemanticToken {
            delta_line: line - previous_line,
            delta_start: if line == previous_line {
                column - previous_column
            } else {
                column
            },
            length,
            token_type,
            token_modifiers_bitset,
        });
        previous_line = line;
        previous_column = column;
        previous_end = column + length;
    }
    result
}

fn lines(text: &str) -> Vec<(usize, &str)> {
    let mut offset = 0;
    text.split_inclusive('\n')
        .map(|line| {
            let start = offset;
            offset += line.len();
            (start, line)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{context::ClientId, handlers::semantic_token::semantic_token};
    use emmylua_code_analysis::{EmmyLuaAnalysis, file_path_to_uri};
    use lsp_types::SemanticTokensResult;

    fn collect(source: &str, extension: &str) -> Vec<(String, u32, u32)> {
        let mut analysis = EmmyLuaAnalysis::new();
        let path = std::env::current_dir()
            .unwrap()
            .join(format!("highlight.{extension}"));
        let uri = file_path_to_uri(&path).unwrap();
        let id = analysis
            .update_file_by_uri(&uri, Some(source.into()))
            .unwrap();
        let result = semantic_token(&analysis, id, false, ClientId::VSCode).unwrap();
        let SemanticTokensResult::Tokens(tokens) = result else {
            panic!("tokens");
        };
        let lines: Vec<_> = source.lines().collect();
        let mut line = 0;
        let mut col = 0;
        let mut previous_end = 0;
        let mut found = vec![];
        for token in tokens.data {
            line += token.delta_line as usize;
            col = if token.delta_line == 0 {
                col + token.delta_start
            } else {
                previous_end = 0;
                token.delta_start
            };
            assert!(col >= previous_end);
            let units: Vec<_> = lines[line].encode_utf16().collect();
            let end = (col + token.length) as usize;
            assert!(end <= units.len(), "token outside line: {token:?}");
            let text = String::from_utf16(&units[col as usize..end]).unwrap();
            assert!(!text.is_empty());
            found.push((text, token.token_type, token.token_modifiers_bitset));
            previous_end = end as u32;
        }
        found
    }

    #[test]
    fn inline_and_cast_types_match_and_unicode_ranges_are_valid() {
        let source = "local emoji = 'СЂСџв„ўвЂљ'; local value: number = 1\r\nlocal result = value as number\r\n---@class Camera\r\nlocal Camera = {}\r\nlocal camera: Camera = {}\r\nlocal other = camera as Camera\r\n--[[first\r\nsecond]]\r\nreturn result";
        let tokens = collect(source, "cua");
        let numbers: Vec<_> = tokens
            .iter()
            .filter(|(text, _, _)| text == "number")
            .collect();
        assert_eq!(numbers.len(), 2, "{tokens:?}");
        assert_eq!(numbers[0], numbers[1]);
        assert_eq!(numbers[0].1, SemanticTokenTypeKind::Type.to_u32());
        let cameras: Vec<_> = tokens
            .iter()
            .filter(|(text, kind, _)| {
                text == "Camera" && *kind == SemanticTokenTypeKind::Type.to_u32()
            })
            .collect();
        assert_eq!(cameras.len(), 2, "{tokens:?}");
        assert_eq!(cameras[0], cameras[1]);
        assert_eq!(
            tokens
                .iter()
                .filter(|(text, kind, _)| text == "as"
                    && *kind == SemanticTokenTypeKind::Keyword.to_u32())
                .count(),
            2
        );
        assert!(
            !tokens
                .iter()
                .any(|(text, _, _)| text.contains("@as") || text.contains("@type"))
        );
    }

    #[test]
    fn symbol_kinds_and_modifiers_match_lua() {
        let cua = "GLOBAL = {}\nlocal M = {}\nlocal function f(value: number): number return value end\nfunction M.draw(value: number) local x = f(value); GLOBAL.x = x end\nreturn M";
        let lua = "GLOBAL = {}\nlocal M = {}\n---@param value number\n---@return number\nlocal function f(value) return value end\n---@param value number\nfunction M.draw(value) local x = f(value); GLOBAL.x = x end\nreturn M";
        let select = |tokens: Vec<(String, u32, u32)>| {
            tokens
                .into_iter()
                .filter(|(name, _, _)| ["GLOBAL", "M", "f", "draw", "x"].contains(&name.as_str()))
                .collect::<Vec<_>>()
        };
        assert_eq!(select(collect(cua, "cua")), select(collect(lua, "lua")));
    }

    #[test]
    fn class_and_alias_tokens_match_documentation_types() {
        let source = "alias Name = string\nclass Camera\n name: Name\n zoom?: number\nend\nlocal camera: Camera = {name = 'ok'}";
        let tokens = collect(source, "cua");
        for name in [
            "class", "alias", "end", "Name", "Camera", "name", "zoom", "string", "number",
        ] {
            assert!(
                tokens.iter().any(|(text, _, _)| text == name),
                "missing {name}: {tokens:?}"
            );
        }
        let types: Vec<_> = tokens
            .iter()
            .filter(|(text, _, _)| text == "Camera")
            .collect();
        assert_eq!(types.len(), 2);
        assert!(tokens.iter().any(
            |(text, kind, _)| text == "number" && *kind == SemanticTokenTypeKind::Type.to_u32()
        ));
    }

    #[test]
    fn incomplete_cua_clears_tokens() {
        assert!(collect("local value:", "cua").is_empty());
        assert!(collect("", "cua").is_empty());
    }
}
