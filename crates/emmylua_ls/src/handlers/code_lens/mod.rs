mod build_code_lens;
mod resolve_code_lens;

use build_code_lens::build_code_lens;
use emmylua_code_analysis::{EmmyLuaAnalysis, FileId};
use lsp_types::{
    ClientCapabilities, CodeLens, CodeLensOptions, CodeLensParams, ServerCapabilities,
};
use resolve_code_lens::resolve_code_lens;
use tokio_util::sync::CancellationToken;

use crate::context::{RequestOutcome, ServerContextSnapshot, analysis_query};

use super::RegisterCapabilities;

pub async fn on_code_lens_handler(
    context: ServerContextSnapshot,
    params: CodeLensParams,
    cancel_token: CancellationToken,
) -> RequestOutcome<Vec<CodeLens>> {
    let uri = params.text_document.uri;
    let cache_key = format!("code_lens:{}", uri.as_str());
    analysis_query(
        context.analysis(),
        context.request_manager(),
        &cache_key,
        Some(cancel_token.clone()),
        move |analysis| {
            let file_id = analysis.get_file_id(&uri)?;
            code_lenses(analysis, file_id)
        },
    )
    .await
}

pub async fn on_resolve_code_lens_handler(
    context: ServerContextSnapshot,
    code_lens: CodeLens,
    _: CancellationToken,
) -> RequestOutcome<CodeLens> {
    let client_id = context
        .workspace_manager()
        .lock()
        .await
        .client_config
        .client_id;
    let fallback = code_lens.clone();
    let result = context
        .analysis()
        .run_blocking(move |analysis| resolve_code_lens(analysis, code_lens, client_id))
        .await
        .unwrap_or(fallback);

    RequestOutcome::Ready(result)
}

pub struct CodeLensCapabilities;

impl RegisterCapabilities for CodeLensCapabilities {
    fn register_capabilities(server_capabilities: &mut ServerCapabilities, _: &ClientCapabilities) {
        server_capabilities.code_lens_provider = Some(CodeLensOptions {
            resolve_provider: Some(true),
        });
    }
}

fn code_lenses(analysis: &EmmyLuaAnalysis, file_id: FileId) -> Option<Vec<CodeLens>> {
    if !analysis.get_emmyrc().code_lens.enable {
        return None;
    }
    let model = analysis.semantic_model(file_id);
    let document = analysis.db.document(file_id)?;
    let lenses = build_code_lens(&model, &document)?;
    Some(
        lenses
            .into_iter()
            .filter_map(|mut lens| {
                if let Some(source) = analysis.cua_document(file_id) {
                    lens.range = source.original_range(lens.range)?;
                }
                Some(lens)
            })
            .collect(),
    )
}

#[cfg(test)]
mod cua_tests {
    use super::*;
    use crate::context::ClientId;
    use emmylua_code_analysis::{Emmyrc, file_path_to_uri};
    use lsp_types::{Location, Position};
    use std::sync::Arc;

    #[test]
    fn cua_argument_hints_and_usage_locations_use_source_coordinates() {
        let mut analysis = EmmyLuaAnalysis::new();
        let mut config = Emmyrc::default();
        config.code_lens.enable = true;
        config.hint.enable = true;
        analysis.update_config(Arc::new(config));
        let root = std::env::current_dir().unwrap();
        analysis.add_main_workspace(root.clone());
        let path = root.join("hints.cua");
        let uri = file_path_to_uri(&path).unwrap();
        let source = "local function resizeShot(size: number) end\nlocal emoji = '🙂'; resizeShot(12)\nresizeShot(24)";
        let id = analysis
            .update_file_by_uri(&uri, Some(source.into()))
            .unwrap();
        let hints =
            crate::handlers::inlay_hint::inlay_hint(&analysis, id, ClientId::VSCode).unwrap();
        assert_eq!(hints.len(), 2, "{hints:?}");
        for (hint, (line, value)) in hints.iter().zip([(1, "12"), (2, "24")]) {
            let text = source.lines().nth(line).unwrap();
            let col = text[..text.find(value).unwrap()].encode_utf16().count() as u32;
            assert_eq!(hint.position, Position::new(line as u32, col));
            let lsp_types::InlayHintLabel::LabelParts(parts) = &hint.label else {
                panic!()
            };
            assert_eq!(parts[0].value, "size:");
            assert_eq!(
                parts[0].location.as_ref().unwrap().range.start,
                hint.position
            );
        }
        let lenses = code_lenses(&analysis, id).unwrap();
        assert_eq!(lenses.len(), 1);
        assert_eq!(lenses[0].range.start, Position::new(0, 15));
        let resolved = resolve_code_lens(&analysis, lenses[0].clone(), ClientId::VSCode).unwrap();
        let command = resolved.command.unwrap();
        assert_eq!(command.title, "2 usages");
        let args = command.arguments.unwrap();
        assert_eq!(args[1], serde_json::json!({"line":0,"character":15}));
        let refs: Vec<Location> = serde_json::from_value(args[2].clone()).unwrap();
        assert_eq!(refs.len(), 2);
        for location in refs {
            assert_eq!(location.uri, uri);
            let text = source
                .lines()
                .nth(location.range.start.line as usize)
                .unwrap();
            let units: Vec<_> = text.encode_utf16().collect();
            assert_eq!(
                String::from_utf16(
                    &units[location.range.start.character as usize
                        ..location.range.end.character as usize]
                )
                .unwrap(),
                "resizeShot"
            );
        }
        analysis.update_file_by_uri(&uri, Some("local value:".into()));
        assert!(code_lenses(&analysis, id).unwrap().is_empty());
        assert!(
            crate::handlers::inlay_hint::inlay_hint(&analysis, id, ClientId::VSCode)
                .unwrap()
                .is_empty()
        );
    }
}
