use std::path::Path;

use emmylua_code_analysis::{FileId, SemanticDatabase};
use lsp_types::{Diagnostic, DiagnosticSeverity};

use super::OutputWriter;

pub struct GccOutputWriter;

impl OutputWriter for GccOutputWriter {
    fn write(&mut self, db: &SemanticDatabase, file_id: FileId, diagnostics: Vec<Diagnostic>) {
        let Some(path) = db.file_path(file_id) else {
            return;
        };
        for diagnostic in diagnostics {
            println!("{}", format_diagnostic(&path, &diagnostic));
        }
    }

    fn finish(&mut self) {}
}

fn format_diagnostic(path: &Path, diagnostic: &Diagnostic) -> String {
    let path = path.to_string_lossy();
    // Ordinary absolute paths work in terminal links, including when the checked
    // workspace differs from the terminal's working directory.
    let path = if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        format!("//{unc}")
    } else {
        path.strip_prefix(r"\\?\").unwrap_or(&path).to_owned()
    }
    .replace('\\', "/");
    let severity = match diagnostic.severity {
        Some(DiagnosticSeverity::ERROR) => "error",
        Some(DiagnosticSeverity::WARNING) => "warning",
        _ => "note",
    };
    let message = diagnostic
        .message
        .split(['\r', '\n'])
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{}:{}:{}: {}: {}",
        path,
        diagnostic.range.start.line + 1,
        diagnostic.range.start.character + 1,
        severity,
        message
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Position, Range};

    #[test]
    fn windows_path_and_multiline_message_are_one_clickable_line() {
        let diagnostic = Diagnostic {
            range: Range::new(Position::new(9, 4), Position::new(10, 7)),
            severity: Some(DiagnosticSeverity::WARNING),
            message: "expected `string`\r\n  found `number`\nmore detail".into(),
            ..Default::default()
        };
        assert_eq!(
            format_diagnostic(Path::new(r"\\?\D:\My Project\file.lua"), &diagnostic),
            "D:/My Project/file.lua:10:5: warning: expected `string` found `number` more detail"
        );
    }

    #[test]
    fn unc_paths_and_notes_are_preserved() {
        let diagnostic = Diagnostic {
            message: "unused".into(),
            severity: Some(DiagnosticSeverity::HINT),
            ..Default::default()
        };
        assert_eq!(
            format_diagnostic(Path::new(r"\\?\UNC\server\share\file.lua"), &diagnostic),
            "//server/share/file.lua:1:1: note: unused"
        );
    }
}
