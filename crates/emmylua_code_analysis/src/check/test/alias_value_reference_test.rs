use super::{check_source, count_by_code};
use crate::{DiagnosticCode, LuaType, LuaTypeNode, VirtualWorkspace};

#[test]
fn literal_alias_references_keep_exact_values() {
    let diagnostics = check_source(
        r#"
local RED = 1
local BLUE = 2
---@alias Color `RED` | `BLUE`
---@param color Color
local function paint(color) end
paint(RED)
paint(BLUE)
paint(3)
"#,
    );
    assert_eq!(
        count_by_code(&diagnostics, DiagnosticCode::ParamTypeMismatch),
        1,
        "{diagnostics:#?}"
    );
}

#[test]
fn forward_namespace_references_ignore_circular_field_annotations() {
    let diagnostics = check_source(
        r#"
api = {}
---@alias Format
---| `api.Format.Small`
---| `api.Format.Nested.Large`
api.Format = {
    Small = 0, ---@type Format
    Nested = { Large = 104 },
}
---@param format Format
local function create(format) end
create(api.Format.Small)
create(api.Format.Nested.Large)
create(105)
"#,
    );
    assert_eq!(
        count_by_code(&diagnostics, DiagnosticCode::ParamTypeMismatch),
        1,
        "{diagnostics:#?}"
    );
}

#[test]
fn value_references_use_alias_definition_scope_across_files() {
    let mut ws = VirtualWorkspace::new();
    ws.def_file(
        "sdk.lua",
        r#"
local VALUE = 'sdk'
---@alias Choice `VALUE`
"#,
    );
    let file = ws.def_file("consumer.lua", "local VALUE = 'consumer'");
    let model = ws.analysis.semantic_model(file);
    let def = model.resolve_type_def("Choice").unwrap();
    assert_eq!(
        model.alias_target(&def),
        Some(LuaType::StringConst(smol_str::SmolStr::new("sdk").into()))
    );
}

#[test]
fn value_references_support_scalar_constants() {
    let diagnostics = check_source(
        r#"
local TEXT = 'valid'
local YES = true
local NEGATIVE = -2
---@alias Choice `TEXT` | `YES` | `NEGATIVE`
---@param value Choice
local function accept(value) end
accept('valid')
accept(true)
accept(-2)
accept(false)
accept('invalid')
"#,
    );
    assert_eq!(
        count_by_code(&diagnostics, DiagnosticCode::ParamTypeMismatch),
        2,
        "{diagnostics:#?}"
    );
}

#[test]
fn generic_backticks_are_not_value_references() {
    let mut ws = VirtualWorkspace::new();
    ws.def(
        r#"
local T = 42
---@class Vehicle
local Vehicle = {}
---@alias Factory<T> fun(name: `T`): T
---@generic T
---@param name `T`
---@return T
function new(name) end
"#,
    );
    assert_eq!(ws.expr_ty("new('Vehicle')"), ws.ty("Vehicle"));
    let file = ws.def("local marker = 1");
    let model = ws.analysis.semantic_model(file);
    let def = model.resolve_type_def("Factory").unwrap();
    assert!(
        model
            .alias_target(&def)
            .unwrap()
            .any_type(|ty| matches!(ty, LuaType::StrTplRef(_)))
    );
}

#[test]
fn literal_alias_does_not_implicitly_accept_flag_combinations() {
    let diagnostics = check_source(
        r#"
local flags = { A = 1, B = 2 }
---@alias Flag `flags.A` | `flags.B`
---@param value Flag
local function use(value) end
use(flags.A)
use(flags.B)
use(flags.A + flags.B)
"#,
    );
    assert_eq!(
        count_by_code(&diagnostics, DiagnosticCode::ParamTypeMismatch),
        1,
        "{diagnostics:#?}"
    );
}

#[test]
fn cyclic_and_missing_value_references_do_not_hide_unrelated_errors() {
    let diagnostics = check_source(
        r#"
api = {}
api.A = api.B
api.B = api.A
---@alias Cycle `api.A`
---@alias Missing `api.NotDefined`
---@param value Cycle
local function cycle(value) end
---@param value Missing
local function missing(value) end
cycle(1)
missing(1)
---@param value string
local function text(value) end
text(42)
"#,
    );
    assert_eq!(
        count_by_code(&diagnostics, DiagnosticCode::ParamTypeMismatch),
        3,
        "{diagnostics:#?}"
    );
}

#[test]
fn referenced_values_refresh_after_dependency_edits() {
    let mut ws = VirtualWorkspace::new();
    ws.def_file(
        "sdk.lua",
        "api = { Value = 1 }\n---@alias Choice `api.Value`",
    );
    let consumer = ws.def_file("consumer.lua", "local value = 1");
    {
        let model = ws.analysis.semantic_model(consumer);
        let def = model.resolve_type_def("Choice").unwrap();
        assert_eq!(model.alias_target(&def), Some(LuaType::IntegerConst(1)));
    }
    ws.def_file(
        "sdk.lua",
        "api = { Value = 2 }\n---@alias Choice `api.Value`",
    );
    let model = ws.analysis.semantic_model(consumer);
    let def = model.resolve_type_def("Choice").unwrap();
    assert_eq!(model.alias_target(&def), Some(LuaType::IntegerConst(2)));
}

#[test]
fn references_follow_local_values_without_using_shadowing_globals() {
    let diagnostics = check_source(
        r#"
VALUE = 99
local VALUE = 1
local COPY = VALUE
local values = { One = COPY }
---@alias Choice `values.One`
---@param value Choice
local function use(value) end
use(1)
use(99)
"#,
    );
    assert_eq!(
        count_by_code(&diagnostics, DiagnosticCode::ParamTypeMismatch),
        1,
        "{diagnostics:#?}"
    );
}

#[test]
fn reference_to_alias_attached_local_table_is_resolved() {
    let diagnostics = check_source(
        r#"
---@alias Choice `values.One` | `values.Two`
local values = { One = 1, Two = 2 }
---@param value Choice
local function use(value) end
use(1)
use(2)
use(3)
"#,
    );
    assert_eq!(
        count_by_code(&diagnostics, DiagnosticCode::ParamTypeMismatch),
        1,
        "{diagnostics:#?}"
    );
}
