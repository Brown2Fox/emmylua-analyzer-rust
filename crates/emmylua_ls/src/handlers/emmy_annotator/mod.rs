mod build_annotator;
mod emmy_annotator_request;

use std::str::FromStr;

use build_annotator::build_annotators;
pub use emmy_annotator_request::*;
use emmylua_code_analysis::{EmmyLuaAnalysis, FileId};
use lsp_types::Uri;
use tokio_util::sync::CancellationToken;

use crate::context::{RequestOutcome, ServerContextSnapshot, analysis_query};

pub async fn on_emmy_annotator_handler(
    context: ServerContextSnapshot,
    params: EmmyAnnotatorParams,
    cancel_token: CancellationToken,
) -> RequestOutcome<Vec<EmmyAnnotator>> {
    let Ok(uri) = Uri::from_str(&params.uri) else {
        return RequestOutcome::Missing;
    };
    let cache_key = format!("annotator:{}", uri.as_str());
    analysis_query(
        context.analysis(),
        context.request_manager(),
        &cache_key,
        Some(cancel_token.clone()),
        move |analysis| {
            let file_id = analysis.get_file_id(&uri)?;
            annotators(analysis, file_id)
        },
    )
    .await
}

fn annotators(analysis: &EmmyLuaAnalysis, file_id: FileId) -> Option<Vec<EmmyAnnotator>> {
    let model = analysis.semantic_model(file_id);
    let document = analysis.db.document(file_id)?;
    let mut result = build_annotators(&model, &document, &analysis.get_emmyrc());
    if let Some(source) = analysis.cua_document(file_id) {
        for annotation in &mut result {
            annotation.ranges = annotation
                .ranges
                .iter()
                .filter_map(|range| source.original_range(*range))
                .filter(|range| range.start < range.end)
                .collect();
        }
        result.retain(|annotation| !annotation.ranges.is_empty());
    }
    Some(result)
}

#[cfg(test)]
mod cua_tests {
    use super::*;
    use emmylua_code_analysis::file_path_to_uri;

    fn collect(source: &str, ext: &str) -> Vec<(u8, String)> {
        let mut analysis = EmmyLuaAnalysis::new();
        let path = std::env::current_dir()
            .unwrap()
            .join(format!("decorations.{ext}"));
        let uri = file_path_to_uri(&path).unwrap();
        let id = analysis
            .update_file_by_uri(&uri, Some(source.into()))
            .unwrap();
        let lines: Vec<_> = source.lines().collect();
        let mut result = vec![];
        for annotation in annotators(&analysis, id).unwrap() {
            for range in annotation.ranges {
                assert_eq!(range.start.line, range.end.line);
                let line = lines[range.start.line as usize];
                let text = if ext == "cua" {
                    let units: Vec<_> = line.encode_utf16().collect();
                    String::from_utf16(
                        &units[range.start.character as usize..range.end.character as usize],
                    )
                    .unwrap()
                } else {
                    // Existing Lua ranges use scalar columns; Cua maps them to LSP UTF-16.
                    line.chars()
                        .skip(range.start.character as usize)
                        .take((range.end.character - range.start.character) as usize)
                        .collect()
                };
                result.push((annotation.typ.clone().into(), text));
            }
        }
        result.sort();
        result
    }

    #[test]
    fn cua_decorations_match_lua_with_typed_parameters_and_unicode() {
        let body = "  local emoji = 'рџ™‚'; ac.use(size)\n  obsShot = ac.create(size)\n  obsShot:update()\nend";
        let lua = format!(
            "local obsShot = nil\n---@param size number\nlocal function resizeShot(size)\n{body}"
        );
        let cua = format!("local obsShot = nil\nlocal function resizeShot(size: number)\n{body}");
        let found = collect(&cua, "cua");
        assert!(found.contains(&(1, "ac".into())), "{found:?}");
        assert!(found.contains(&(3, "obsShot".into())), "{found:?}");
        assert_eq!(found, collect(&lua, "lua"));
        assert!(collect("local value:", "cua").is_empty());
    }
}
