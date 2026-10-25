use super::{check_source, count_by_code};
use crate::DiagnosticCode;

#[test]
fn optional_object_fields_keep_declared_types() {
    let diagnostics = check_source(
        r#"
---@class Pose
---@field position number
local Pose = {}
---@return Pose?
local function getPose() return {position = 1} end
---@param value number
local function consume(value) end
local pose = getPose()
if pose == nil then return end
consume(pose.position)
---@type Pose?
local current = nil
---@param value Pose
local function update(value)
    current = value
    consume(current.position)
end
"#,
    );
    assert_eq!(
        count_by_code(&diagnostics, DiagnosticCode::ParamTypeMismatch),
        0,
        "{diagnostics:#?}"
    );
}

#[test]
fn declared_numeric_field_assignment_keeps_contract() {
    for (value, expected) in [("value * 0.75", 0), ("'invalid'", 1)] {
        let source = format!(
            r#"
---@class Input
---@field zoom number
local M = {{}}
---@type Input
M.input = {{zoom = 0.0}}
---@param value number
function M.update(value)
    local input = M.input
    input.zoom = 0
    input.zoom = {value}
end
return M
"#
        );
        let diagnostics = check_source(&source);
        assert_eq!(
            count_by_code(&diagnostics, DiagnosticCode::AssignTypeMismatch),
            expected,
            "{diagnostics:#?}"
        );
    }
}

#[test]
fn computed_array_index_is_not_a_missing_class_field() {
    let diagnostics = check_source(
        r#"
---@class Unrelated
---@field name string
local Unrelated = {}
local corners = {1, 2, 3, 4}
for i = 1, 4 do
    local next = corners[i % 4 + 1]
end
---@type Unrelated
local instance = {}
local missing = instance.missing
"#,
    );
    assert_eq!(
        count_by_code(&diagnostics, DiagnosticCode::UndefinedField),
        1,
        "{diagnostics:#?}"
    );
}

#[test]
fn unguarded_union_member_still_reports_mismatch() {
    for other in ["nil", "Other"] {
        let source = format!(
            r#"
---@class Pose
---@field position number
local Pose = {{}}
---@class Other
local Other = {{}}
---@param value number
local function consume(value) end
---@param pose Pose|{other}
local function update(pose)
    consume(pose.position)
end
"#
        );
        let diagnostics = check_source(&source);
        assert_eq!(
            count_by_code(&diagnostics, DiagnosticCode::ParamTypeMismatch),
            1,
            "{diagnostics:#?}"
        );
    }
}

#[test]
fn captured_unknown_assignment_is_not_nil() {
    for annotation in ["unknown?", "Shot?"] {
        let source = format!(
            r#"
---@class Shot
local Shot = {{}}
function Shot:dispose() end
---@return Shot
local function create() return {{}} end
local shot = nil ---@type {annotation}
local function dispose()
    if shot == nil then return end
    shot:dispose()
    shot = nil
end
local function resize()
    dispose()
    shot = create()
    shot:dispose()
end
"#
        );
        let diagnostics = check_source(&source);
        assert_eq!(
            count_by_code(&diagnostics, DiagnosticCode::NeedCheckNil),
            0,
            "{diagnostics:#?}"
        );
        assert_eq!(
            count_by_code(&diagnostics, DiagnosticCode::ParamTypeMismatch),
            0,
            "{diagnostics:#?}"
        );
    }
}

#[test]
fn table_with_function_declaration_is_callable() {
    let diagnostics = check_source(
        r#"
local api = {}
---@type table<string, string>
api.storage = {}
---@param key string
---@return number
function api.storage(key) return 1 end
local value = api.storage('key')
---@type number
local result = value
"#,
    );
    assert_eq!(
        count_by_code(&diagnostics, DiagnosticCode::CallNonCallable),
        0,
        "{diagnostics:#?}"
    );
}

#[test]
fn unresolved_call_result_does_not_restore_previous_nil() {
    let diagnostics = check_source(
        r#"
---@class Shot
local Shot = {}
function Shot:dispose() end
---@param first number
---@param second boolean
---@param third string
---@return Shot
---@overload fun(first: number, second: string)
local function create(first, second, third) end
local shot = nil ---@type unknown?
local function resize()
    shot = create(1, false, 0)
    shot:dispose()
end
"#,
    );
    assert_eq!(
        count_by_code(&diagnostics, DiagnosticCode::NeedCheckNil),
        0,
        "{diagnostics:#?}"
    );
}

#[test]
fn non_callable_table_and_union_still_report() {
    let diagnostics = check_source(
        r#"
local api = {}
api.storage = {}
api.storage('key')
---@param value number|fun()
local function run(value)
    value()
end
"#,
    );
    assert_eq!(
        count_by_code(&diagnostics, DiagnosticCode::CallNonCallable),
        2,
        "{diagnostics:#?}"
    );
}

#[test]
fn explicit_nullable_call_result_still_reports() {
    let diagnostics = check_source(
        r#"
---@class Shot
local Shot = {}
function Shot:dispose() end
---@return Shot?
local function create() return nil end
local shot = nil ---@type unknown?
local function resize()
    shot = create()
    shot:dispose()
end
"#,
    );
    assert_eq!(
        count_by_code(&diagnostics, DiagnosticCode::NeedCheckNil),
        1,
        "{diagnostics:#?}"
    );
}

#[test]
fn class_annotation_on_namespace_member_matches_local_class() {
    for (target, initializer) in [
        ("local camera", "nil"),
        ("api.Camera", "nil"),
        ("api.Camera", "{position = 1}"),
    ] {
        let source = format!(
            r#"
local api = {{}}
---@class Camera
---@field position number
{target} = {initializer}
---@param value number
local function consume(value) end
consume({access}.position)
"#,
            access = if target.starts_with("local") {
                "camera"
            } else {
                "api.Camera"
            }
        );
        let diagnostics = check_source(&source);
        assert_eq!(
            count_by_code(&diagnostics, DiagnosticCode::NeedCheckNil),
            0,
            "{diagnostics:#?}"
        );
        assert_eq!(
            count_by_code(&diagnostics, DiagnosticCode::ParamTypeMismatch),
            0,
            "{diagnostics:#?}"
        );
    }
}

#[test]
fn unannotated_nil_namespace_member_still_reports() {
    let diagnostics = check_source(
        r#"
local api = {}
api.Camera = nil
api.Camera.position = 1
"#,
    );
    assert_eq!(
        count_by_code(&diagnostics, DiagnosticCode::NeedCheckNil),
        1,
        "{diagnostics:#?}"
    );
}

#[test]
fn sdk_namespace_declarations_work_across_files() {
    use crate::VirtualWorkspace;
    use lsp_types::NumberOrString;
    use tokio_util::sync::CancellationToken;

    let mut ws = VirtualWorkspace::new();
    ws.def_file(
        "sdk.lua",
        r#"
api = {}
---@class Camera
---@field position number
api.Camera = nil
---@type table<string, string>
api.storage = {}
---@param key string
---@return number
function api.storage(key) return 1 end
"#,
    );
    let file = ws.def_file(
        "consumer.lua",
        r#"
api.Camera.position = 1
local result = api.storage('key')
api.storage(false)
"#,
    );
    let diagnostics = ws
        .analysis
        .diagnose_file(file, CancellationToken::new())
        .unwrap();
    for code in [
        DiagnosticCode::CallNonCallable,
        DiagnosticCode::NeedCheckNil,
    ] {
        assert!(
            !diagnostics
                .iter()
                .any(|d| d.code == Some(NumberOrString::String(code.get_name().to_string()))),
            "{diagnostics:#?}"
        );
    }
    assert_eq!(
        diagnostics
            .iter()
            .filter(|d| d.code
                == Some(NumberOrString::String(
                    DiagnosticCode::ParamTypeMismatch.get_name().to_string()
                )))
            .count(),
        1,
        "{diagnostics:#?}"
    );
}

#[test]
fn annotated_nil_member_preserves_declared_type_across_files() {
    use crate::VirtualWorkspace;
    use lsp_types::NumberOrString;
    use tokio_util::sync::CancellationToken;
    let mut ws = VirtualWorkspace::new();
    ws.def_file(
        "sdk.lua",
        r#"
api = {}
---@class Camera
---@field position number

---@type Camera
api.Camera = nil
---@type Camera?
api.OptionalCamera = nil
"#,
    );
    let file = ws.def_file(
        "consumer.lua",
        r#"
api.Camera.position = 1
api.OptionalCamera.position = 1
"#,
    );
    let diagnostics = ws
        .analysis
        .diagnose_file(file, CancellationToken::new())
        .unwrap();
    let nils: Vec<_> = diagnostics
        .iter()
        .filter(|d| {
            d.code
                == Some(NumberOrString::String(
                    DiagnosticCode::NeedCheckNil.get_name().to_string(),
                ))
        })
        .collect();
    assert_eq!(nils.len(), 1, "{diagnostics:#?}");
    assert!(
        nils[0].message.contains("OptionalCamera"),
        "{diagnostics:#?}"
    );
}
