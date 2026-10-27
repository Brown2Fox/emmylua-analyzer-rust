mod goto_def_definition;
mod goto_label;
mod goto_module_file;
mod goto_string;

use emmylua_code_analysis::{EmmyLuaAnalysis, FileId, LuaType, SemanticId};
use emmylua_parser::{LuaAstNode, LuaAstToken, LuaStringToken, LuaTokenKind};
use lsp_types::{
    ClientCapabilities, GotoDefinitionParams, GotoDefinitionResponse, OneOf, Position,
    ServerCapabilities,
};
use rowan::TokenAtOffset;
use tokio_util::sync::CancellationToken;

use super::RegisterCapabilities;
use crate::context::{RequestOutcome, ServerContextSnapshot, snapshot_query};

pub async fn on_goto_definition_handler(
    context: ServerContextSnapshot,
    params: GotoDefinitionParams,
    cancel_token: CancellationToken,
) -> RequestOutcome<GotoDefinitionResponse> {
    let uri = params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;
    snapshot_query(context.analysis(), cancel_token, move |analysis| {
        let file_id = analysis.get_file_id(&uri)?;
        definition(analysis, file_id, position)
    })
    .await
}

pub fn definition(
    analysis: &EmmyLuaAnalysis,
    file_id: FileId,
    position: Position,
) -> Option<GotoDefinitionResponse> {
    navigate(analysis, file_id, position, false)
}

pub fn type_definition(
    analysis: &EmmyLuaAnalysis,
    file_id: FileId,
    position: Position,
) -> Option<GotoDefinitionResponse> {
    navigate(analysis, file_id, position, true)
}

fn navigate(
    analysis: &EmmyLuaAnalysis,
    file_id: FileId,
    position: Position,
    type_only: bool,
) -> Option<GotoDefinitionResponse> {
    let position = match analysis.cua_document(file_id) {
        Some(document) => document.generated_position(position)?,
        None => position,
    };
    let response = definition_generated(analysis, file_id, position, type_only)?;
    map_response(analysis, file_id, response)
}

fn map_response(
    analysis: &EmmyLuaAnalysis,
    source: FileId,
    response: GotoDefinitionResponse,
) -> Option<GotoDefinitionResponse> {
    let map_range = |uri: &lsp_types::Uri, range| match analysis
        .get_file_id(uri)
        .and_then(|id| analysis.cua_document(id))
    {
        Some(document) => document.original_range(range),
        None => Some(range),
    };
    let map_location = |mut location: lsp_types::Location| {
        location.range = map_range(&location.uri, location.range)?;
        Some(location)
    };
    Some(match response {
        GotoDefinitionResponse::Scalar(location) => {
            GotoDefinitionResponse::Scalar(map_location(location)?)
        }
        GotoDefinitionResponse::Array(locations) => {
            GotoDefinitionResponse::Array(locations.into_iter().filter_map(map_location).collect())
        }
        GotoDefinitionResponse::Link(links) => GotoDefinitionResponse::Link(
            links
                .into_iter()
                .filter_map(|mut link| {
                    link.target_range = map_range(&link.target_uri, link.target_range)?;
                    link.target_selection_range =
                        map_range(&link.target_uri, link.target_selection_range)?;
                    if let Some(document) = analysis.cua_document(source) {
                        if let Some(range) = link.origin_selection_range {
                            link.origin_selection_range = Some(document.original_range(range)?);
                        }
                    }
                    Some(link)
                })
                .collect(),
        ),
    })
}

fn definition_generated(
    analysis: &EmmyLuaAnalysis,
    file_id: FileId,
    position: Position,
    type_only: bool,
) -> Option<GotoDefinitionResponse> {
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
            if left.kind() == LuaTokenKind::TkName.into()
                || (left.kind() == LuaTokenKind::TkLeftBracket.into()
                    && right.kind() == LuaTokenKind::TkInt.into())
            {
                left
            } else {
                right
            }
        }
        TokenAtOffset::None => {
            return None;
        }
    };

    if type_only {
        let info = model.semantic_info(token.clone().into())?;
        if let Some(decl @ SemanticId::TypeDef(_)) = info.decl {
            return goto_def_definition::goto_def_definition(&model, &analysis.db, &decl, &token);
        }
        let mut pending = vec![info.typ];
        let mut locations = Vec::new();
        while let Some(ty) = pending.pop() {
            let id = match ty {
                LuaType::Ref(id) | LuaType::Def(id) => Some(id),
                LuaType::Generic(generic) => Some(generic.get_base_type_id_ref().clone()),
                LuaType::Union(union) => {
                    pending.extend(union.into_vec());
                    None
                }
                _ => None,
            };
            if let Some(id) = id {
                for def in model.type_defs_of(&id).iter() {
                    if let Some(document) = analysis.db.document(def.file_id) {
                        if let (Some(uri), Some(range)) =
                            (document.get_uri(), document.to_lsp_range(def.name_range))
                        {
                            let location = lsp_types::Location { uri, range };
                            if !locations.contains(&location) {
                                locations.push(location);
                            }
                        }
                    }
                }
            }
        }
        return (!locations.is_empty()).then_some(GotoDefinitionResponse::Array(locations));
    }

    // 1. Label definition (goto / label).
    if let Some(response) = goto_label::goto_label_definition(&model, &analysis.db, &token) {
        return Some(response);
    }

    // 2. String token: require module file / string template reference.
    if let Some(string_token) = LuaStringToken::cast(token.clone()) {
        if let Some(response) =
            goto_module_file::goto_module_file(&analysis.db, file_id, string_token.clone())
        {
            return Some(response);
        }
        if let Some(response) =
            goto_string::goto_str_tpl_ref_definition(&model, &analysis.db, string_token)
        {
            return Some(response);
        }
        return None;
    }

    // 2.5 Doc description reference / `@see`.
    if let Some(response) = goto_def_definition::goto_doc_definition(
        &model,
        &analysis.db,
        &token,
        position_offset,
        &analysis.get_emmyrc(),
    ) {
        return Some(response);
    }

    // 3. Semantic declarations (decl / member / typedef).
    let decl = model.find_decl(token.clone().into())?;
    goto_def_definition::goto_def_definition(&model, &analysis.db, &decl, &token)
}

pub async fn on_type_definition_handler(
    context: ServerContextSnapshot,
    params: lsp_types::request::GotoTypeDefinitionParams,
    cancel_token: CancellationToken,
) -> RequestOutcome<GotoDefinitionResponse> {
    let uri = params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;
    snapshot_query(context.analysis(), cancel_token, move |analysis| {
        type_definition(analysis, analysis.get_file_id(&uri)?, position)
    })
    .await
}

pub struct DefinitionCapabilities;

impl RegisterCapabilities for DefinitionCapabilities {
    fn register_capabilities(server_capabilities: &mut ServerCapabilities, _: &ClientCapabilities) {
        server_capabilities.definition_provider = Some(OneOf::Left(true));
        server_capabilities.type_definition_provider =
            Some(lsp_types::TypeDefinitionProviderCapability::Simple(true));
    }
}

#[cfg(test)]
mod cua_tests {
    use super::*;
    use emmylua_code_analysis::file_path_to_uri;
    fn locations(response: Option<GotoDefinitionResponse>) -> Vec<lsp_types::Location> {
        match response.expect("navigation result") {
            GotoDefinitionResponse::Scalar(location) => vec![location],
            GotoDefinitionResponse::Array(locations) => locations,
            other => panic!("unexpected response: {other:?}"),
        }
    }
    #[test]
    fn maps_symbols_and_types_in_both_directions() {
        let mut analysis = EmmyLuaAnalysis::new();
        let root = std::env::current_dir().unwrap();
        analysis.add_main_workspace(root.clone());
        let cua_path = root.join("navigation.cua");
        let lua_path = root.join("navigation-sdk.lua");
        let cua_uri = file_path_to_uri(&cua_path).unwrap();
        let lua_uri = file_path_to_uri(&lua_path).unwrap();
        let cua = "local function identity(x: string): string return x end\n---@class Camera\n---@field zoom number\nlocal Camera = {}\nlocal camera: Camera = {}\nlocal external: External = {}\nlocal emoji = '🙂'; local result = identity('ok')\nreturn camera\n";
        let lua = "---@class External\nlocal External = {}\n---@type Camera\nlocal camera = {}\nreturn camera\n";
        analysis.update_files_by_path(vec![
            (cua_path, Some(cua.into())),
            (lua_path, Some(lua.into())),
        ]);
        let cua_id = analysis.get_file_id(&cua_uri).unwrap();
        let lua_id = analysis.get_file_id(&lua_uri).unwrap();
        let column = cua
            .lines()
            .nth(6)
            .unwrap()
            .split("identity")
            .next()
            .unwrap()
            .encode_utf16()
            .count() as u32;
        let found = locations(definition(&analysis, cua_id, Position::new(6, column + 1)));
        assert_eq!(found[0].uri, cua_uri);
        assert_eq!(found[0].range.start, Position::new(0, 15));
        assert_eq!(found[0].range.end, Position::new(0, 23));
        let found = locations(definition(&analysis, cua_id, Position::new(4, 16)));
        assert_eq!(found[0].range.start, Position::new(1, 10));
        let found = locations(definition(&analysis, cua_id, Position::new(5, 18)));
        assert_eq!(found[0].uri, lua_uri);
        assert_eq!(found[0].range.start, Position::new(0, 10));
        let found = locations(definition(&analysis, lua_id, Position::new(2, 10)));
        assert_eq!(found[0].uri, cua_uri);
        assert_eq!(found[0].range.start, Position::new(1, 10));
        let found = locations(type_definition(&analysis, cua_id, Position::new(7, 9)));
        assert_eq!(found[0].uri, cua_uri);
        assert_eq!(found[0].range.start, Position::new(1, 10));
        let found = locations(type_definition(&analysis, lua_id, Position::new(4, 9)));
        assert_eq!(found[0].uri, cua_uri);
        assert_eq!(found[0].range.start, Position::new(1, 10));
        let cast_source = "local n: number = 1\n---@class Camera\n---@field zoom number\nlocal Camera = {}\n---@alias View Camera\nlocal value: unknown = {}\nlocal camera = value as View\nreturn camera.zoom";
        analysis.update_file_by_uri(&cua_uri, Some(cast_source.into()));
        let found = locations(definition(&analysis, cua_id, Position::new(6, 25)));
        assert_eq!(found[0].range.start, Position::new(4, 10));
        let found = locations(definition(&analysis, cua_id, Position::new(7, 16)));
        assert_eq!(found[0].range.start, Position::new(2, 10));
        analysis.update_file_by_uri(&cua_uri, Some("local x:".into()));
        assert!(definition(&analysis, cua_id, Position::new(0, 6)).is_none());
        assert!(type_definition(&analysis, cua_id, Position::new(0, 6)).is_none());
    }
}
