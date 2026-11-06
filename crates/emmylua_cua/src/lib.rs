//! Shared Cua frontend. Inline types are projected into EmmyLua annotations for
//! semantic analysis and erased without moving runtime code for Lua execution.

use emmylua_parser::{
    LexerConfig, LuaAstNode, LuaDocTagType, LuaDocType, LuaExpr, LuaLanguageLevel, LuaLexer,
    LuaParser, LuaTokenData, LuaTokenKind as K, ParserConfig, Reader,
};
use std::ops::Range;

#[derive(Debug, Clone)]
pub struct Error {
    pub offset: usize,
    pub message: String,
}

#[derive(Debug)]
pub struct Output {
    pub lua: String,
    pub analysis_lua: String,
    source_offsets: Vec<usize>,
    generated_offsets: Vec<usize>,
    pub type_markers: Vec<Range<usize>>,
}

impl Output {
    /// Only ranges backed by consecutive original bytes are safe to highlight.
    /// Generated annotations and erased copies of inline types are excluded.
    pub fn original_token_range(&self, range: Range<usize>) -> Option<Range<usize>> {
        if range.is_empty() || range.end > self.analysis_lua.len() {
            return None;
        }
        let start = self.original_offset(range.start);
        for (delta, generated) in range.clone().enumerate() {
            let original = start + delta;
            if self.original_offset(generated) != original
                || self.generated_offset(original) != generated
            {
                return None;
            }
        }
        Some(start..start + range.len())
    }
    pub fn original_offset(&self, offset: usize) -> usize {
        self.source_offsets[offset.min(self.source_offsets.len() - 1)]
    }
    pub fn generated_offset(&self, offset: usize) -> usize {
        self.generated_offsets[offset.min(self.generated_offsets.len() - 1)]
    }
}

struct Annotation {
    offset: usize,
    text: String,
    types: Vec<(Range<usize>, Range<usize>)>,
}

struct Frontend<'a> {
    source: &'a str,
    all_tokens: Vec<LuaTokenData>,
    tokens: Vec<LuaTokenData>,
    erased: Vec<Range<usize>>,
    annotations: Vec<Annotation>,
    pending_types: Vec<Range<usize>>,
    casts: Vec<usize>,
    declaration_markers: Vec<Range<usize>>,
}

pub fn transpile(source: &str, level: LuaLanguageLevel) -> Result<Output, Error> {
    let all_tokens = LuaLexer::new(Reader::new(source), LexerConfig::new(level), None).tokenize();
    let tokens = all_tokens
        .iter()
        .copied()
        .filter(|t| {
            !matches!(
                t.kind,
                K::TkWhitespace
                    | K::TkEndOfLine
                    | K::TkShortComment
                    | K::TkLongComment
                    | K::TkShebang
            )
        })
        .collect();
    let mut frontend = Frontend {
        source,
        all_tokens,
        tokens,
        erased: vec![],
        annotations: vec![],
        pending_types: vec![],
        casts: vec![],
        declaration_markers: vec![],
    };
    let mut i = 0;
    while i < frontend.tokens.len() {
        i = match frontend.kind(i) {
            K::TkName if frontend.is_declaration(i) => frontend.declaration(i)?,
            K::TkLocal if frontend.kind(i + 1) != K::TkFunction => frontend.local(i)?,
            K::TkFunction => frontend.function(i)?,
            K::TkName if frontend.is_cast(i) => frontend.cast(i)?,
            _ => i + 1,
        };
    }
    let mut bytes = source.as_bytes().to_vec();
    for range in &frontend.erased {
        for byte in &mut bytes[range.clone()] {
            if !matches!(*byte, b'\r' | b'\n') {
                *byte = b' ';
            }
        }
    }
    let lua = String::from_utf8(bytes).expect("erasure replaces complete UTF-8 type ranges");
    let tree = LuaParser::parse(&lua, ParserConfig::with_level(level));
    if let Some(error) = tree.get_errors().first() {
        return Err(Error {
            offset: u32::from(error.range.start()) as usize,
            message: error.message.clone(),
        });
    }
    for keyword in &frontend.casts {
        let operand_end = frontend.end(keyword - 1);
        let inner = tree
            .get_red_root()
            .descendants()
            .filter_map(LuaExpr::cast)
            .filter(|expr| u32::from(expr.get_range().end()) as usize == operand_end)
            .min_by_key(|expr| expr.get_range().start());
        if let Some(inner) = inner {
            frontend.annotations.push(Annotation {
                offset: u32::from(inner.get_range().start()) as usize,
                text: "((".into(),
                types: vec![],
            });
        } else {
            return Err(frontend.error(*keyword, "expected an expression before 'as'"));
        }
    }
    frontend.annotations.sort_by_key(|a| a.offset);
    let mut analysis_lua = String::new();
    let mut source_offsets = vec![];
    let mut generated_offsets = vec![0; source.len() + 1];
    let mut type_offsets = vec![];
    let mut cursor = 0;
    for annotation in frontend.annotations {
        let offset = annotation.offset;
        for (original, generated) in generated_offsets
            .iter_mut()
            .enumerate()
            .take(offset)
            .skip(cursor)
        {
            *generated = analysis_lua.len() + original - cursor;
        }
        analysis_lua.push_str(&lua[cursor..offset]);
        source_offsets.extend(cursor..offset);
        let base = analysis_lua.len();
        analysis_lua.push_str(&annotation.text);
        source_offsets.extend(std::iter::repeat_n(offset, annotation.text.len()));
        for (generated, original) in annotation.types {
            // Include the end boundary so diagnostic ranges end at the type,
            // rather than jumping back to the declaration anchor.
            for delta in 0..=original.len() {
                type_offsets.push((original.start + delta, base + generated.start + delta));
            }
        }
        cursor = offset;
    }
    for (original, generated) in generated_offsets.iter_mut().enumerate().skip(cursor) {
        *generated = analysis_lua.len() + original - cursor;
    }
    analysis_lua.push_str(&lua[cursor..]);
    source_offsets.extend(cursor..=source.len());
    for (original, generated) in type_offsets {
        generated_offsets[original] = generated;
        source_offsets[generated] = original;
    }
    Ok(Output {
        lua,
        analysis_lua,
        source_offsets,
        generated_offsets,
        type_markers: frontend
            .erased
            .iter()
            .filter(|range| {
                source[range.start..].starts_with(':') || source[range.start..].starts_with("as")
            })
            .map(|range| {
                let length = if source[range.start..].starts_with("as") {
                    2
                } else {
                    1
                };
                range.start..range.start + length
            })
            .chain(frontend.declaration_markers)
            .collect(),
    })
}

impl Frontend<'_> {
    fn is_declaration(&self, i: usize) -> bool {
        matches!(self.text(i), "class" | "alias")
            && self.source[..self.start(i)]
                .rsplit('\n')
                .next()
                .unwrap_or("")
                .trim()
                .is_empty()
            && (self.kind(i + 1) == K::TkName
                || (self.text(i) == "class"
                    && self.kind(i + 1) == K::TkLeftParen
                    && self.kind(i + 2) == K::TkName
                    && self.kind(i + 3) == K::TkRightParen
                    && self.kind(i + 4) == K::TkName))
    }

    fn declaration(&mut self, anchor: usize) -> Result<usize, Error> {
        let is_class = self.text(anchor) == "class";
        let mut text = String::from("\n---@");
        let mut mappings = vec![];
        let mut markers = vec![];
        let mut copy = |text: &mut String, range: Range<usize>| {
            let start = text.len();
            text.push_str(&self.source[range.clone()]);
            mappings.push((start..text.len(), range));
        };
        copy(&mut text, self.start(anchor)..self.end(anchor));
        text.push(' ');
        let header_start = self.start(anchor + 1);
        let header_end = self.source[header_start..]
            .find(['\r', '\n'])
            .map_or(self.source.len(), |n| header_start + n);
        let mut i = anchor + 1;
        if is_class {
            // Class headers use EmmyLua's name, generic, attribute and inheritance grammar.
            let end = self.source[header_start..header_end]
                .find("--")
                .map_or(header_end, |n| header_start + n);
            copy(&mut text, header_start..end);
            text.push('\n');
            i = self
                .tokens
                .partition_point(|token| token.range.start_offset < header_end);
            while self.kind(i) != K::TkEnd {
                if self.kind(i) == K::TkEof {
                    return Err(self.error(i, "expected 'end' after class fields"));
                }
                let field_start = self.start(i);
                let mut depth = 0;
                let mut colon = i;
                loop {
                    if self.kind(colon) == K::TkEof
                        || self.kind(colon) == K::TkEnd
                        || self.source[field_start..self.start(colon)].contains('\n')
                    {
                        return Err(self.error(colon, "expected ':' after class field name"));
                    }
                    match self.kind(colon) {
                        K::TkLeftBracket => depth += 1,
                        K::TkRightBracket => depth -= 1,
                        K::TkColon if depth == 0 => break,
                        _ => {}
                    }
                    colon += 1;
                }
                markers.push(self.start(colon)..self.end(colon));
                let (_, next, end) = self.ty(colon + 1)?;
                text.push_str("---@field ");
                copy(&mut text, field_start..self.start(colon));
                text.push(' ');
                copy(&mut text, self.start(colon + 1)..end);
                text.push('\n');
                i = next;
                if matches!(self.kind(i), K::TkComma | K::TkSemicolon) {
                    i += 1;
                }
            }
        } else {
            while !matches!(self.kind(i), K::TkAssign | K::TkEof) && self.start(i) < header_end {
                i += 1;
            }
            if self.kind(i) != K::TkAssign {
                return Err(self.error(i, "expected '=' in alias declaration"));
            }
            markers.push(self.start(i)..self.end(i));
            copy(&mut text, header_start..self.start(i));
            text.push(' ');
            let (_, next, end) = self.ty(i + 1)?;
            copy(&mut text, self.start(i + 1)..end);
            text.push('\n');
            i = next;
        }
        let end = if is_class {
            self.end(i)
        } else {
            self.end(i - 1)
        };
        // An explicitly typed table directly after its class is its implementation.
        // Other following statements must not inherit the declaration's annotation.
        let class_name = if self.kind(anchor + 1) == K::TkName {
            self.text(anchor + 1)
        } else {
            self.text(anchor + 4)
        };
        let binds_implementation = is_class
            && self.kind(i + 1) == K::TkLocal
            && self.kind(i + 2) == K::TkName
            && self.kind(i + 3) == K::TkColon
            && self.text(i + 4) == class_name
            && self.kind(i + 5) == K::TkAssign
            && self.kind(i + 6) == K::TkLeftBrace;
        if !binds_implementation {
            text.push_str("do end\n");
        }
        let parsed = LuaParser::parse(&text, ParserConfig::default());
        if !parsed.get_errors().is_empty() {
            return Err(self.error(anchor, "invalid class or alias declaration"));
        }
        self.declaration_markers.extend(markers);
        self.annotations.push(Annotation {
            offset: if binds_implementation {
                self.start(i + 1)
            } else {
                self.start(anchor)
            },
            text,
            types: mappings,
        });
        self.erased.push(self.start(anchor)..end);
        if is_class {
            self.declaration_markers.push(self.start(i)..self.end(i));
            Ok(i + 1)
        } else {
            Ok(i)
        }
    }

    fn is_cast(&self, i: usize) -> bool {
        self.text(i) == "as"
            && i > 0
            && !matches!(
                self.kind(i + 1),
                K::TkAssign | K::TkDot | K::TkColon | K::TkLeftParen
            )
            && matches!(
                self.kind(i - 1),
                K::TkName
                    | K::TkRightParen
                    | K::TkRightBracket
                    | K::TkRightBrace
                    | K::TkString
                    | K::TkLongString
                    | K::TkInt
                    | K::TkFloat
                    | K::TkComplex
                    | K::TkTrue
                    | K::TkFalse
                    | K::TkNil
                    | K::TkDots
                    | K::TkEnd
            )
    }

    fn cast(&mut self, keyword: usize) -> Result<usize, Error> {
        let (ty, next, end) = self.ty(keyword + 1)?;
        if !matches!(
            self.kind(next),
            K::TkRightParen
                | K::TkRightBracket
                | K::TkRightBrace
                | K::TkComma
                | K::TkSemicolon
                | K::TkEof
                | K::TkEnd
                | K::TkThen
                | K::TkDo
                | K::TkElse
                | K::TkElseIf
                | K::TkUntil
                | K::TkLocal
                | K::TkReturn
                | K::TkName
                | K::TkFunction
                | K::TkIf
                | K::TkFor
                | K::TkWhile
                | K::TkRepeat
                | K::TkBreak
                | K::TkGoto
                | K::TkDbColon
        ) {
            return Err(self.error(
                next,
                "parenthesize the cast before continuing the expression: (expression as Type)",
            ));
        }
        self.erased.push(self.start(keyword)..end);
        // A long-comment delimiter must not occur inside a string-literal type.
        let mut equals = String::new();
        while ty.contains(&format!("]{equals}]")) {
            equals.push('=');
        }
        let prefix = format!(") --[{equals}[@as ");
        let type_start = prefix.len();
        self.annotations.push(Annotation {
            offset: self.start(keyword),
            text: format!("{prefix}{ty}]{equals}])"),
            types: vec![(
                type_start..type_start + ty.len(),
                self.start(keyword + 1)..end,
            )],
        });
        self.casts.push(keyword);
        Ok(next)
    }

    fn kind(&self, i: usize) -> K {
        self.tokens.get(i).map_or(K::TkEof, |t| t.kind)
    }
    fn start(&self, i: usize) -> usize {
        self.tokens
            .get(i)
            .map_or(self.source.len(), |t| t.range.start_offset)
    }
    fn end(&self, i: usize) -> usize {
        self.tokens
            .get(i)
            .map_or(self.source.len(), |t| t.range.end_offset())
    }
    fn text(&self, i: usize) -> &str {
        &self.source[self.start(i)..self.end(i)]
    }
    fn error(&self, i: usize, message: &str) -> Error {
        Error {
            offset: self.start(i),
            message: message.into(),
        }
    }

    // The existing EmmyLua grammar remains the single definition of type syntax.
    fn ty(&self, i: usize) -> Result<(String, usize, usize), Error> {
        let start = self.start(i);
        let line = self.source[start..]
            .split(['\r', '\n'])
            .next()
            .unwrap_or("");
        let prefix = "---@type ";
        let doc = format!("{prefix}{line}\n");
        let tree = LuaParser::parse(&doc, ParserConfig::default());
        let ty = tree
            .get_red_root()
            .descendants()
            .find_map(LuaDocTagType::cast)
            .and_then(|tag| tag.get_type_list().next())
            .ok_or_else(|| self.error(i, "expected an inline type"))?;
        let range = ty.get_range();
        let from = u32::from(range.start()) as usize;
        let to = u32::from(range.end()) as usize;
        let text = &doc[from..to];
        let validation = LuaParser::parse(&format!("{prefix}{text}\n"), ParserConfig::default());
        if text.is_empty() || !validation.get_errors().is_empty() {
            return Err(self.error(i, "invalid inline type"));
        }
        let end = start + to - prefix.len();
        let next = self
            .tokens
            .partition_point(|token| token.range.start_offset < end);
        Ok((text.to_string(), next, end))
    }

    fn annotated_type(&mut self, colon: usize) -> Result<(String, usize), Error> {
        let (ty, next, end) = self.ty(colon + 1)?;
        self.erased.push(self.start(colon)..end);
        self.pending_types.push(self.start(colon + 1)..end);
        Ok((ty, next))
    }

    fn annotate(&mut self, anchor: usize, tags: Vec<String>) -> Result<(), Error> {
        if tags.is_empty() {
            return Ok(());
        }
        let offset = self.start(anchor);
        for token in self
            .all_tokens
            .iter()
            .rev()
            .filter(|t| t.range.end_offset() <= offset)
        {
            if matches!(token.kind, K::TkWhitespace | K::TkEndOfLine) {
                continue;
            }
            if !matches!(token.kind, K::TkShortComment | K::TkLongComment) {
                break;
            }
            let prefix = self.source[..token.range.start_offset]
                .rsplit('\n')
                .next()
                .unwrap_or("");
            if !prefix.trim().is_empty() {
                break;
            }
            let comment = &self.source[token.range.start_offset..token.range.end_offset()];
            for tag in &tags {
                let key = if tag.starts_with("---@param ") {
                    tag.split_whitespace().take(2).collect::<Vec<_>>().join(" ")
                } else {
                    tag.split_whitespace().next().unwrap_or("").to_string()
                };
                if comment
                    .split_once(key.trim_start_matches('-'))
                    .is_some_and(|(_, tail)| tail.starts_with(char::is_whitespace))
                {
                    return Err(self.error(
                        anchor,
                        "inline type duplicates an EmmyLua annotation; keep one declaration",
                    ));
                }
            }
        }
        let line_prefix = self.source[..offset].rsplit('\n').next().unwrap_or("");
        let separator = if line_prefix.trim().is_empty() {
            ""
        } else {
            "\n"
        };
        let text = format!("{separator}{}\n", tags.join("\n"));
        let tree = LuaParser::parse(&text, ParserConfig::default());
        let ranges = tree
            .get_red_root()
            .descendants()
            .filter_map(LuaDocType::cast)
            .filter(|ty| {
                !ty.syntax()
                    .ancestors()
                    .skip(1)
                    .any(|node| LuaDocType::can_cast(node.kind().into()))
            })
            .map(|ty| {
                let r = ty.get_range();
                u32::from(r.start()) as usize..u32::from(r.end()) as usize
            });
        let types = ranges.zip(self.pending_types.drain(..)).collect();
        self.annotations.push(Annotation {
            offset,
            text,
            types,
        });
        Ok(())
    }

    fn local(&mut self, anchor: usize) -> Result<usize, Error> {
        let mut i = anchor + 1;
        let mut types = vec![];
        while self.kind(i) == K::TkName {
            i += 1;
            if self.kind(i) == K::TkLt && self.kind(i + 2) == K::TkGt {
                i += 3;
            }
            if self.kind(i) == K::TkColon {
                let (ty, next) = self.annotated_type(i)?;
                types.push(Some(ty));
                i = next;
            } else {
                types.push(None);
            }
            if self.kind(i) != K::TkComma {
                break;
            }
            i += 1;
        }
        if types.iter().any(Option::is_some) {
            if types.iter().any(Option::is_none) {
                return Err(self.error(
                    anchor,
                    "split mixed typed and inferred locals into separate declarations",
                ));
            }
            self.annotate(
                anchor,
                vec![format!(
                    "---@type {}",
                    types.into_iter().flatten().collect::<Vec<_>>().join(", ")
                )],
            )?;
        }
        Ok(i.max(anchor + 1))
    }

    fn function(&mut self, function: usize) -> Result<usize, Error> {
        let anonymous = self.kind(function + 1) == K::TkLeftParen;
        let anchor = if function > 0 && self.kind(function - 1) == K::TkLocal {
            function - 1
        } else {
            function
        };
        let mut i = function + 1;
        while matches!(self.kind(i), K::TkName | K::TkDot | K::TkColon) {
            i += 1;
        }
        if self.kind(i) != K::TkLeftParen {
            return Ok(function + 1);
        }
        i += 1;
        let mut tags = vec![];
        while matches!(self.kind(i), K::TkName | K::TkDots) {
            let name = self.text(i).to_string();
            i += 1;
            if self.kind(i) == K::TkColon {
                let (ty, next) = self.annotated_type(i)?;
                tags.push(format!("---@param {name} {ty}"));
                i = next;
            }
            if self.kind(i) != K::TkComma {
                break;
            }
            i += 1;
        }
        if self.kind(i) != K::TkRightParen {
            return Err(self.error(i, "expected ')' after function parameters"));
        }
        i += 1;
        if self.kind(i) == K::TkColon {
            let colon = i;
            i += 1;
            let tuple = self.kind(i) == K::TkLeftParen;
            if tuple {
                i += 1;
            }
            let mut returns = vec![];
            let mut end;
            loop {
                let (ty, next, type_end) = self.ty(i)?;
                self.pending_types.push(self.start(i)..type_end);
                returns.push(ty);
                i = next;
                end = type_end;
                if self.kind(i) != K::TkComma {
                    break;
                }
                i += 1;
            }
            if tuple {
                if self.kind(i) != K::TkRightParen {
                    return Err(self.error(i, "expected ')' after return types"));
                }
                end = self.end(i);
                i += 1;
            }
            self.erased.push(self.start(colon)..end);
            tags.push(format!("---@return {}", returns.join(", ")));
        }
        if anonymous && !tags.is_empty() {
            return Err(self.error(
                function,
                "inline types on anonymous functions are not supported yet; use a named function",
            ));
        }
        self.annotate(anchor, tags)?;
        Ok(i)
    }
}
