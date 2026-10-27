use super::*;
use crate::{CheckConfig, Emmyrc};
use std::{str::FromStr, sync::Arc};

fn uri() -> Uri {
    Uri::from_str("file:///C:/workspace/input.cua").unwrap()
}
fn errors(analysis: &EmmyLuaAnalysis, id: FileId) -> Vec<Diagnostic> {
    analysis
        .diagnose_file_with_config(id, Arc::new(CheckConfig::new(&Emmyrc::default())))
        .unwrap()
        .into_iter()
        .filter(|d| {
            matches!(
                d.severity,
                Some(DiagnosticSeverity::ERROR | DiagnosticSeverity::WARNING)
            )
        })
        .collect()
}

#[test]
fn checks_inline_types_and_maps_diagnostics_to_original_utf16_ranges() {
    let mut analysis = EmmyLuaAnalysis::new();
    let source = "local function take(n: number): number return n end\nlocal text = '🙂'; take('bad')\nreturn text";
    let id = analysis
        .update_file_by_uri(&uri(), Some(source.into()))
        .unwrap();
    let diagnostics = errors(&analysis, id);
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(
        diagnostics[0].code,
        Some(NumberOrString::String("param-type-mismatch".into()))
    );
    let start = source.find("'bad'").unwrap();
    assert_eq!(
        diagnostics[0].range,
        Range::new(
            source_position(source, start),
            source_position(source, start + 5)
        )
    );
}

#[test]
fn unknown_inline_type_errors_point_to_type_not_declaration() {
    let mut analysis = EmmyLuaAnalysis::new();
    let source = "local value: Missing = {}";
    let id = analysis
        .update_file_by_uri(&uri(), Some(source.into()))
        .unwrap();
    let diagnostics = errors(&analysis, id);
    assert!(
        diagnostics.iter().any(|d| d.range.start.character == 13),
        "{diagnostics:#?}"
    );
}

#[test]
fn edits_recover_without_stale_types_or_lua_syntax_noise() {
    let mut analysis = EmmyLuaAnalysis::new();
    let id = analysis
        .update_file_by_uri(&uri(), Some("local n: number = 1\nreturn n".into()))
        .unwrap();
    assert!(errors(&analysis, id).is_empty());
    analysis.update_file_by_uri(&uri(), Some("local n: ".into()));
    let diagnostics = errors(&analysis, id);
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].code,
        Some(NumberOrString::String("cua-syntax-error".into()))
    );
    assert!(
        analysis
            .cua_document(id)
            .unwrap()
            .generated_position(Position::new(0, 6))
            .is_none()
    );
    analysis.update_file_by_uri(&uri(), Some("local n: string = 'ok'\nreturn n".into()));
    assert!(errors(&analysis, id).is_empty());
    analysis.remove_file_by_uri(&uri());
    assert!(!analysis.has_cua_documents());
}

#[test]
fn batch_reload_and_config_changes_keep_original_sources() {
    let mut analysis = EmmyLuaAnalysis::new();
    let source = "local n: number = 1\nreturn n";
    let other = Uri::from_str("file:///C:/workspace/other.lua").unwrap();
    analysis.update_files_by_uri(vec![
        (uri(), Some(source.into())),
        (other, Some("return 1".into())),
    ]);
    let id = analysis.get_file_id(&uri()).unwrap();
    analysis.update_config(Arc::new(Emmyrc::default()));
    assert_eq!(analysis.cua_document(id).unwrap().source, source);
    assert!(errors(&analysis, id).is_empty());
    analysis.reload_workspace_files(
        vec![(
            uri_to_file_path(&uri()).unwrap(),
            Some("local n: number = 'bad'".into()),
        )],
        vec![(uri(), source.into())],
    );
    let id = analysis.get_file_id(&uri()).unwrap();
    assert_eq!(analysis.cua_document(id).unwrap().source, source);
    assert!(errors(&analysis, id).is_empty());
}

#[test]
fn indexes_cua_files_by_default() {
    let (include, _, _) = crate::calculate_include_and_exclude(&Emmyrc::default());
    assert!(include.iter().any(|pattern| pattern == "**/*.cua"));
}

#[test]
fn cast_type_diagnostics_and_offsets_use_original_source() {
    let mut analysis = EmmyLuaAnalysis::new();
    let source = "local s = '🙂'; return (s as MissingType)";
    let id = analysis
        .update_file_by_uri(&uri(), Some(source.into()))
        .unwrap();
    let start = source.find("MissingType").unwrap();
    let end = start + "MissingType".len();
    let diagnostics = errors(&analysis, id);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.range
                == Range::new(source_position(source, start), source_position(source, end))),
        "{diagnostics:#?}"
    );
    let projection = analysis
        .cua_document(id)
        .unwrap()
        .projection
        .as_ref()
        .unwrap();
    for offset in start..=end {
        assert_eq!(
            projection.original_offset(projection.generated_offset(offset)),
            offset
        );
    }
    analysis.update_file_by_uri(&uri(), Some("local s = '🙂'; return (s as )".into()));
    assert_eq!(
        errors(&analysis, id)[0].code,
        Some(NumberOrString::String("cua-syntax-error".into()))
    );
    analysis.update_file_by_uri(&uri(), Some("local s = '🙂'; return (s as string)".into()));
    assert!(errors(&analysis, id).is_empty());
}

#[test]
fn strict_tables_accept_inline_types_and_report_original_names() {
    let mut analysis = EmmyLuaAnalysis::new();
    let source = "local typed: table<string, number> = {}\nlocal inferred = {}";
    let id = analysis
        .update_file_by_uri(&uri(), Some(source.into()))
        .unwrap();
    let mut config = Emmyrc::default();
    config.strict.no_implicit_table = true;
    let diagnostics = analysis
        .diagnose_file_with_config(id, Arc::new(CheckConfig::new(&config)))
        .unwrap();
    let diagnostics: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.code == Some(NumberOrString::String("no-implicit-table".into())))
        .collect();
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(
        diagnostics[0].range,
        Range::new(Position::new(1, 6), Position::new(1, 14))
    );
}

#[test]
fn dictionary_values_report_missing_fields_on_cua_literals() {
    let mut analysis = EmmyLuaAnalysis::new();
    analysis.init_std_lib(None);
    analysis.add_main_workspace(std::path::PathBuf::from("C:/workspace"));
    let source = "---@class ParamDef\n---@field kind string\n---@field options? string[]\nlocal cameraParams: table<string, ParamDef> = { camera = {} }\nlocal controlParams: table<string, ParamDef> = { control = {kind = 'cbox'} }";
    let id = analysis
        .update_file_by_uri(&uri(), Some(source.into()))
        .unwrap();
    let diagnostics = errors(&analysis, id);
    let missing: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.code == Some(NumberOrString::String("missing-fields".into())))
        .collect();
    assert_eq!(missing.len(), 1, "{diagnostics:#?}");
    let offset = source.find("{}").unwrap();
    assert_eq!(
        missing[0].range,
        Range::new(
            source_position(source, offset),
            source_position(source, offset + 2)
        )
    );
    assert!(missing[0].message.contains("kind"));
}

#[test]
fn native_declarations_check_fields_and_map_unknown_types() {
    let mut analysis = EmmyLuaAnalysis::new();
    let source = "class Params\n name: string\n optional?: number\nend\nalias Config = Params\nlocal config: Config = {}";
    let id = analysis
        .update_file_by_uri(&uri(), Some(source.into()))
        .unwrap();
    let diagnostics = errors(&analysis, id);
    assert!(
        diagnostics.iter().any(
            |d| d.code == Some(NumberOrString::String("missing-fields".into()))
                && d.range.start.line == 5
                && d.message.contains("name")
        ),
        "{diagnostics:?}"
    );
    let source = "class Params\n value: MissingType\nend";
    analysis.update_file_by_uri(&uri(), Some(source.into()));
    let diagnostics = errors(&analysis, id);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.range.start == Position::new(1, 8) && d.range.end == Position::new(1, 19)),
        "{diagnostics:?}"
    );
}
