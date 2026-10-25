use super::{check_source, check_source_with_emmyrc, count_by_code};
use crate::{DiagnosticCode, Emmyrc};

fn strict_count(source: &str) -> usize {
    let mut config = Emmyrc::default();
    config.strict.no_implicit_any = true;
    count_by_code(
        &check_source_with_emmyrc(source, config),
        DiagnosticCode::NoImplicitAny,
    )
}

#[test]
fn disabled_by_default() {
    assert_eq!(
        count_by_code(
            &check_source("local function f(value) return value end"),
            DiagnosticCode::NoImplicitAny
        ),
        0
    );
}

#[test]
fn reports_each_untyped_parameter_and_varargs() {
    assert_eq!(
        strict_count("local function f(value, player, ...) return value end"),
        3
    );
}

#[test]
fn accepts_explicit_types_including_any_unknown_and_generics() {
    assert_eq!(
        strict_count(
            r#"
---@generic T
---@param value T
---@param dynamic any
---@param opaque unknown
---@param optional? number
---@param ... string
local function f(value, dynamic, opaque, optional, ...) return value end
"#
        ),
        0
    );
}

#[test]
fn partial_annotation_reports_only_missing_parameter() {
    assert_eq!(
        strict_count("---@param value number\nlocal function f(value, other) end"),
        1
    );
}

#[test]
fn accepts_local_function_type_and_alias() {
    for ty in ["number", "any", "unknown"] {
        assert_eq!(
            strict_count(&format!(
                "---@alias Callback fun(value: {ty})\n---@type Callback\nlocal f = function(value) end"
            )),
            0,
            "{ty}"
        );
    }
}

#[test]
fn accepts_typed_callback_parameters() {
    for ty in ["number", "any", "unknown"] {
        assert_eq!(
            strict_count(&format!(
                "---@param callback fun(value: {ty})\nlocal function consume(callback) end\nconsume(function(value) end)"
            )),
            0,
            "{ty}"
        );
    }
}

#[test]
fn accepts_inherited_method_parameters_and_implicit_self() {
    assert_eq!(
        strict_count(
            r#"
---@class Parent
---@field update fun(self: Parent, dt: number)

---@class Child: Parent
local M = {}
function M:update(dt) end
"#
        ),
        0
    );
}

#[test]
fn untyped_methods_and_unused_parameters_are_not_exempt() {
    assert_eq!(strict_count("local M = {}\nfunction M:run(_unused) end"), 1);
}

#[test]
fn no_parameters_and_inferred_locals_need_no_annotations() {
    assert_eq!(
        strict_count("local count = 10\nlocal function f() return count end"),
        0
    );
}

#[test]
fn configuration_and_inline_suppression_work() {
    let mut config = Emmyrc::default();
    config.strict.no_implicit_any = true;
    config
        .diagnostics
        .disable
        .push(DiagnosticCode::NoImplicitAny);
    assert_eq!(
        count_by_code(
            &check_source_with_emmyrc("local function f(x) end", config),
            DiagnosticCode::NoImplicitAny
        ),
        0
    );
    assert_eq!(
        strict_count("---@diagnostic disable-next-line: no-implicit-any\nlocal function f(x) end"),
        0
    );
    let mut config = Emmyrc::default();
    config
        .diagnostics
        .enables
        .push(DiagnosticCode::NoImplicitAny);
    let diagnostics = check_source_with_emmyrc("local function f(x) end", config);
    let diagnostic = diagnostics
        .iter()
        .find(|d| d.code == DiagnosticCode::NoImplicitAny)
        .unwrap();
    assert_eq!(diagnostic.severity, lsp_types::DiagnosticSeverity::ERROR);
    assert_eq!(diagnostic.range.len(), rowan::TextSize::from(1));
}

#[test]
fn meta_files_are_not_checked() {
    assert_eq!(strict_count("---@meta\nlocal function f(value) end"), 0);
}

#[test]
fn strict_setting_deserializes_and_defaults_off() {
    let enabled: Emmyrc = serde_json::from_str(r#"{"strict":{"noImplicitAny":true}}"#).unwrap();
    assert!(enabled.strict.no_implicit_any);
    let default: Emmyrc = serde_json::from_str("{}").unwrap();
    assert!(!default.strict.no_implicit_any);
}

#[test]
fn library_signatures_are_consumed_without_checking_library_bodies() {
    use crate::{
        SemanticDatabase, SemanticModel, WorkspaceFolder,
        check::{CheckConfig, check_file},
    };
    use std::{str::FromStr, sync::Arc};
    let mut config = Emmyrc::default();
    config.strict.no_implicit_any = true;
    let config = Arc::new(config);
    let mut db = SemanticDatabase::new();
    db.update_config(config.clone());
    db.add_main_workspace("C:/ws".into());
    db.add_library_workspace(&WorkspaceFolder::new("C:/sdk".into(), true));
    let library = db.set_file_content(
        &lsp_types::Uri::from_str("file:///C:/sdk/api.lua").unwrap(),
        Some(
            r#"
---@param callback fun(value: number)
function sdkSubscribe(callback) end
function untypedLibraryFunction(value) end
"#
            .to_string(),
        ),
    );
    let main = db.set_file_content(
        &lsp_types::Uri::from_str("file:///C:/ws/main.lua").unwrap(),
        Some("sdkSubscribe(function(value) end)\nlocal function untyped(value) end".to_string()),
    );
    let check_config = Arc::new(CheckConfig::new(&config));
    assert!(check_file(&SemanticModel::new(&db, library), check_config.clone()).is_empty());
    assert_eq!(
        count_by_code(
            &check_file(&SemanticModel::new(&db, main), check_config),
            DiagnosticCode::NoImplicitAny
        ),
        1
    );
}

#[test]
fn an_untyped_callback_does_not_hide_missing_parameter_types() {
    assert_eq!(
        strict_count(
            "---@param callback function\nlocal function consume(callback) end\nconsume(function(value) end)"
        ),
        1
    );
}

#[test]
fn nested_functions_are_checked_independently() {
    assert_eq!(
        strict_count(
            "---@param outer number\nlocal function f(outer)\nreturn function(inner) return outer + inner end\nend"
        ),
        1
    );
}

#[test]
fn explicit_dynamic_types_in_member_signatures_are_allowed() {
    for ty in ["any", "unknown"] {
        assert_eq!(
            strict_count(&format!(
                "---@class Api\n---@field handle fun(value: {ty})\nlocal M = {{}}\nfunction M.handle(value) end"
            )),
            0
        );
    }
}

#[test]
fn contextual_function_types_still_check_the_function_body() {
    let mut config = Emmyrc::default();
    config.strict.no_implicit_any = true;
    let diagnostics = check_source_with_emmyrc(
        r#"
---@param value number
local function numeric(value) end
---@type fun(value: string)
local callback = function(value)
    numeric(value)
end
"#,
        config,
    );
    assert_eq!(
        count_by_code(&diagnostics, DiagnosticCode::NoImplicitAny),
        0
    );
    assert_eq!(
        count_by_code(&diagnostics, DiagnosticCode::ParamTypeMismatch),
        1,
        "{diagnostics:?}"
    );
}
