mod build_hover;
pub(crate) mod desc;
mod keyword_hover;
pub(crate) mod render;

use super::RegisterCapabilities;
use crate::context::{RequestOutcome, ServerContextSnapshot, analysis_query};
use build_hover::build_semantic_info_hover;
use emmylua_code_analysis::{EmmyLuaAnalysis, FileId};
use emmylua_parser::{LuaAstNode, LuaDocType, LuaTokenKind};
use keyword_hover::{hover_keyword, is_keyword};
use lsp_types::{
    ClientCapabilities, Hover, HoverContents, HoverParams, HoverProviderCapability, MarkupContent,
    Position, ServerCapabilities,
};
use rowan::TokenAtOffset;
use tokio_util::sync::CancellationToken;
// Old DbIndex-version hover (temporary reuse before batch 3 signature_helper / batch 4 completion migration;

pub async fn on_hover(
    context: ServerContextSnapshot,
    params: HoverParams,
    cancel_token: CancellationToken,
) -> RequestOutcome<Hover> {
    let uri = params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;
    let cache_key = format!("hover:{}", uri.as_str());
    analysis_query(
        context.analysis(),
        context.request_manager(),
        &cache_key,
        Some(cancel_token.clone()),
        move |analysis| {
            let file_id = analysis.get_file_id(&uri)?;
            hover(analysis, file_id, position)
        },
    )
    .await
}

pub fn hover(analysis: &EmmyLuaAnalysis, file_id: FileId, position: Position) -> Option<Hover> {
    let position = if let Some(document) = analysis.cua_document(file_id) {
        document.generated_position(position)?
    } else {
        position
    };
    let mut result = hover_generated(analysis, file_id, position)?;
    if let Some(document) = analysis.cua_document(file_id)
        && let Some(range) = result.range
    {
        result.range = Some(document.original_range(range)?);
    }
    Some(result)
}

fn hover_generated(
    analysis: &EmmyLuaAnalysis,
    file_id: FileId,
    position: Position,
) -> Option<Hover> {
    if !analysis.get_emmyrc().hover.enable {
        return None;
    }
    let model = analysis.semantic_model(file_id);
    let document = analysis.db.document(file_id)?;
    let root = model.chunk()?;
    let position_offset =
        document.get_offset(position.line as usize, position.character as usize)?;

    if position_offset > root.syntax().text_range().end() {
        return None;
    }

    let token = match root.syntax().token_at_offset(position_offset) {
        TokenAtOffset::Single(token) => token,
        TokenAtOffset::Between(left, right) => {
            if matches!(
                right.kind().into(),
                LuaTokenKind::TkDot
                    | LuaTokenKind::TkColon
                    | LuaTokenKind::TkLeftBracket
                    | LuaTokenKind::TkRightBracket
                    | LuaTokenKind::TkWhitespace
            ) {
                left
            } else {
                right
            }
        }
        TokenAtOffset::None => return None,
    };
    match token {
        keywords if is_keyword(keywords.clone()) => Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: lsp_types::MarkupKind::Markdown,
                value: hover_keyword(keywords.clone()),
            }),
            range: document.to_lsp_range(keywords.text_range()),
        }),
        literal
            if matches!(
                literal.kind().into(),
                LuaTokenKind::TkString | LuaTokenKind::TkLongString
            ) =>
        {
            Some(Hover {
                contents: HoverContents::Markup(MarkupContent {
                    kind: lsp_types::MarkupKind::Markdown,
                    value: literal.text().to_string(),
                }),
                range: document.to_lsp_range(literal.text_range()),
            })
        }
        _ => build_semantic_info_hover(&model, &analysis.db, token.clone(), token.text_range())
            .or_else(|| {
                analysis.cua_document(file_id)?;
                let doc_type = token.parent_ancestors().find_map(LuaDocType::cast)?;
                let typ = model.doc_type_lua_rich(doc_type.get_syntax_id());
                Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: lsp_types::MarkupKind::Markdown,
                        value: format!("```lua\n{}\n```", render::humanize(&model, &typ)),
                    }),
                    range: document.to_lsp_range(doc_type.get_range()),
                })
            }),
    }
}

pub struct HoverCapabilities;

#[cfg(test)]
mod cua_tests {
    use super::*;
    use std::str::FromStr;

    fn text(hover: &Hover) -> &str {
        match &hover.contents {
            HoverContents::Markup(content) => &content.value,
            _ => panic!("expected markup"),
        }
    }

    #[test]
    fn open_class_hover_keeps_nested_writes_out_of_root_fields() {
        for (ext, source, pos) in [
            (
                "cua",
                "class Textures\n txHDR?: number\nend\nclass Params: table<string, any>\n textures: Textures\n shader: string\nend\nlocal p: Params = {textures = {txHDR = nil}, shader = ''}\np.textures.txHDR = nil",
                Position::new(3, 8),
            ),
            (
                "lua",
                "---@class Textures\n---@field txHDR number?\n---@class Params: table<string, any>\n---@field textures Textures\n---@field shader string\nlocal p = {textures = {txHDR = nil}, shader = ''}\np.textures.txHDR = nil",
                Position::new(2, 12),
            ),
        ] {
            let mut analysis = EmmyLuaAnalysis::new();
            analysis.init_std_lib(None);
            analysis.add_main_workspace(std::path::PathBuf::from("C:/hover"));
            let uri = lsp_types::Uri::from_str(&format!("file:///C:/hover/test.{ext}")).unwrap();
            let id = analysis
                .update_file_by_uri(&uri, Some(source.into()))
                .unwrap();
            let result = hover(&analysis, id, pos).unwrap();
            let rendered = text(&result);
            assert!(rendered.contains("[string]: any"), "{ext}: {rendered}");
            assert!(rendered.contains("textures: Textures"), "{ext}: {rendered}");
            assert!(!rendered.contains("txHDR:"), "{ext}: {rendered}");
        }
    }

    #[test]
    fn cua_hover_maps_declarations_uses_and_inline_types() {
        let mut analysis = EmmyLuaAnalysis::new();
        let uri = lsp_types::Uri::from_str("file:///C:/test.cua").unwrap();
        let source = "local function double(value: number): number return value * 2 end\nlocal result: number = double(4)\nreturn result";
        let id = analysis
            .update_file_by_uri(&uri, Some(source.into()))
            .unwrap();
        for position in [
            Position::new(0, 23),
            Position::new(0, 30),
            Position::new(0, 39),
            Position::new(1, 8),
            Position::new(2, 9),
        ] {
            let result = hover(&analysis, id, position)
                .unwrap_or_else(|| panic!("Cua hover at {position:?}"));
            assert!(text(&result).contains("number"), "{position:?} {result:#?}");
            let range = result.range.unwrap();
            assert_eq!(range.start.line, position.line, "{result:#?}");
            assert!(
                range.start <= position && range.end >= position,
                "{position:?} {range:?}"
            );
        }
        analysis.update_file_by_uri(&uri, Some("local value:".into()));
        assert!(hover(&analysis, id, Position::new(0, 6)).is_none());
        analysis.update_file_by_uri(
            &uri,
            Some("local value: string = 'ok'\nreturn value".into()),
        );
        assert!(text(&hover(&analysis, id, Position::new(1, 9)).unwrap()).contains("string"));
    }

    #[test]
    fn cua_hover_uses_utf16_positions_after_emoji() {
        let mut analysis = EmmyLuaAnalysis::new();
        let uri = lsp_types::Uri::from_str("file:///C:/test.cua").unwrap();
        let source = "local value: number = 1\nlocal s = '🙂'; print(value)";
        let id = analysis
            .update_file_by_uri(&uri, Some(source.into()))
            .unwrap();
        let result = hover(&analysis, id, Position::new(1, 22)).unwrap();
        assert!(text(&result).contains("number"));
        assert_eq!(
            result.range.unwrap(),
            lsp_types::Range::new(Position::new(1, 22), Position::new(1, 27))
        );
    }

    #[test]
    fn cua_inline_type_hover_reads_emmylua_libraries() {
        let mut analysis = EmmyLuaAnalysis::new();
        let library = lsp_types::Uri::from_str("file:///C:/sdk.d.lua").unwrap();
        analysis.update_file_by_uri(
            &library,
            Some("---@meta\n---@class Camera\n---@field zoom number\nlocal Camera = {}".into()),
        );
        let uri = lsp_types::Uri::from_str("file:///C:/test.cua").unwrap();
        let id = analysis
            .update_file_by_uri(
                &uri,
                Some("local camera: Camera = {}\nreturn camera".into()),
            )
            .unwrap();
        let result = hover(&analysis, id, Position::new(0, 16)).unwrap();
        assert!(
            text(&result).contains("Camera") && text(&result).contains("zoom"),
            "{result:#?}"
        );
        assert_eq!(
            result.range.unwrap(),
            lsp_types::Range::new(Position::new(0, 14), Position::new(0, 20))
        );
    }
}

impl RegisterCapabilities for HoverCapabilities {
    fn register_capabilities(server_capabilities: &mut ServerCapabilities, _: &ClientCapabilities) {
        server_capabilities.hover_provider = Some(HoverProviderCapability::Simple(true));
    }
}
