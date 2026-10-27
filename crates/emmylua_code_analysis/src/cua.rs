//! Original Cua documents and their semantic projections.
use crate::{EmmyLuaAnalysis, FileId, uri_to_file_path};
use emmylua_cua::{Error, Output, transpile};
use emmylua_parser::LineIndex;
use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Position, Range, Uri};

#[derive(Debug)]
pub struct CuaDocument {
    pub source: String,
    pub projection: Result<Output, Error>,
}

pub fn is_cua_uri(uri: &Uri) -> bool {
    uri_to_file_path(uri).is_some_and(|path| path.extension().is_some_and(|ext| ext == "cua"))
}

impl CuaDocument {
    /// LSP input positions use UTF-16; the existing semantic document API uses
    /// Unicode scalar columns internally.
    pub fn generated_position(&self, position: Position) -> Option<Position> {
        let output = self.projection.as_ref().ok()?;
        let offset = source_offset(&self.source, position)?;
        let generated = output.generated_offset(offset);
        let (line, character) = LineIndex::parse(&output.analysis_lua)
            .get_line_col((generated as u32).into(), &output.analysis_lua)?;
        Some(Position::new(line as u32, character as u32))
    }

    pub fn original_range(&self, range: Range) -> Option<Range> {
        let output = self.projection.as_ref().ok()?;
        let index = LineIndex::parse(&output.analysis_lua);
        let original = |pos: Position| {
            let offset = index.get_offset(
                pos.line as usize,
                pos.character as usize,
                &output.analysis_lua,
            )?;
            Some(output.original_offset(u32::from(offset) as usize))
        };
        let start = original(range.start)?;
        let end = original(range.end)?;
        Some(Range::new(
            source_position(&self.source, start.min(end)),
            source_position(&self.source, start.max(end)),
        ))
    }

    pub fn error_diagnostic(&self) -> Option<Diagnostic> {
        let error = self.projection.as_ref().err()?;
        let start = error.offset.min(self.source.len());
        let end = self
            .source
            .get(start..)?
            .chars()
            .next()
            .map_or(start, |ch| start + ch.len_utf8());
        Some(Diagnostic {
            range: Range::new(
                source_position(&self.source, start),
                source_position(&self.source, end),
            ),
            severity: Some(DiagnosticSeverity::ERROR),
            code: Some(NumberOrString::String("cua-syntax-error".into())),
            source: Some("Cua".into()),
            message: error.message.clone(),
            ..Default::default()
        })
    }
}

impl EmmyLuaAnalysis {
    pub fn cua_document(&self, file_id: FileId) -> Option<&CuaDocument> {
        self.cua_documents.get(&self.get_uri(file_id)?)
    }

    pub fn has_cua_documents(&self) -> bool {
        !self.cua_documents.is_empty()
    }

    pub(crate) fn prepare_cua(&mut self, uri: &Uri, text: Option<String>) -> Option<String> {
        if !is_cua_uri(uri) {
            return text;
        }
        let Some(source) = text else {
            self.cua_documents.remove(uri);
            return None;
        };
        let projection = transpile(&source, self.emmyrc.runtime.version.get_language_level());
        // On incomplete input, publish one frontend error instead of parsing
        // typed syntax as Lua or retaining a stale semantic snapshot.
        let generated = projection
            .as_ref()
            .map(|output| output.analysis_lua.clone())
            .unwrap_or_default();
        self.cua_documents
            .insert(uri.clone(), CuaDocument { source, projection });
        Some(generated)
    }
}

fn source_offset(text: &str, position: Position) -> Option<usize> {
    let index = LineIndex::parse(text);
    let start = u32::from(index.get_line_offset(position.line as usize)?) as usize;
    let mut column = 0;
    for (offset, ch) in text[start..].char_indices() {
        if column == position.character {
            return Some(start + offset);
        }
        if ch == '\n' || column > position.character {
            return None;
        }
        column += ch.len_utf16() as u32;
    }
    (column == position.character).then_some(text.len())
}

fn source_position(text: &str, offset: usize) -> Position {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    let prefix = &text[..offset];
    Position::new(
        prefix.bytes().filter(|b| *b == b'\n').count() as u32,
        prefix
            .rsplit('\n')
            .next()
            .unwrap_or("")
            .encode_utf16()
            .count() as u32,
    )
}

#[cfg(test)]
mod tests;
