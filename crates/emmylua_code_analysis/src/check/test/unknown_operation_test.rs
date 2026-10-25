use super::{check_source, check_source_with_emmyrc, count_by_code};
use crate::{DiagnosticCode, Emmyrc};

fn strict_count(source: &str) -> usize {
    let mut config = Emmyrc::default();
    config.strict.check_unknown = true;
    count_by_code(
        &check_source_with_emmyrc(source, config),
        DiagnosticCode::UnknownOperation,
    )
}

fn expression_source(ty: &str, expr: &str) -> String {
    format!("---@param value {ty}\nlocal function probe(value) return {expr} end")
}

#[test]
fn opt_in_only() {
    assert_eq!(
        count_by_code(
            &check_source(&expression_source("unknown", "value + 1")),
            DiagnosticCode::UnknownOperation
        ),
        0
    );
}

#[test]
fn rejects_type_dependent_operations() {
    for expr in [
        "value()",
        "value.field",
        "value[1]",
        "value + 1",
        "1 - value",
        "value * 2",
        "value / 2",
        "value // 2",
        "value % 2",
        "value ^ 2",
        "value .. 'x'",
        "value < 1",
        "1 >= value",
        "-value",
        "#value",
        "~value",
        "value & 1",
        "value | 1",
        "value ~ 1",
        "value << 1",
        "value >> 1",
    ] {
        assert_eq!(
            strict_count(&expression_source("unknown", expr)),
            1,
            "{expr}"
        );
    }
}

#[test]
fn any_remains_an_explicit_escape_hatch() {
    for expr in ["value()", "value.field", "value + 1", "#value", "value < 1"] {
        assert_eq!(strict_count(&expression_source("any", expr)), 0, "{expr}");
    }
}

#[test]
fn allows_boolean_operations_and_equality() {
    for expr in [
        "not value",
        "value and true",
        "value or 0",
        "value == nil",
        "value ~= 1",
        "type(value)",
    ] {
        assert_eq!(
            strict_count(&expression_source("unknown", expr)),
            0,
            "{expr}"
        );
    }
}

#[test]
fn narrows_number_and_function_before_use() {
    assert_eq!(
        strict_count(
            r#"
---@param value unknown
local function probe(value)
    if type(value) == 'number' then return value + 1 end
    if type(value) == 'function' then return value() end
end
"#
        ),
        0
    );
}

#[test]
fn table_narrowing_allows_lookup_but_result_still_needs_a_type() {
    assert_eq!(
        strict_count(
            r#"
---@param value unknown
local function probe(value)
    if type(value) == 'table' then
        local field = value.field
        return field + 1
    end
end
"#
        ),
        1
    );
}

#[test]
fn nil_and_truthiness_checks_do_not_prove_a_concrete_type() {
    for condition in ["value", "value ~= nil"] {
        assert_eq!(
            strict_count(&format!(
                "---@param value unknown\nlocal function probe(value) if {condition} then return value + 1 end end"
            )),
            1,
            "{condition}"
        );
    }
}

#[test]
fn validates_writes_and_computed_keys() {
    assert_eq!(
        strict_count(
            r#"
---@param value unknown
---@param key unknown
local function probe(value, key)
    value.field = 1
    local t = {}
    t[key] = 1
end
"#
        ),
        2
    );
}

#[test]
fn avoids_cascading_reports_in_access_chains() {
    assert_eq!(
        strict_count(&expression_source("unknown", "value.a.b()")),
        1
    );
}

#[test]
fn aliases_and_nullable_unknown_are_checked() {
    assert_eq!(
        strict_count(
            "---@alias Opaque unknown\n---@param value Opaque\nlocal function probe(value) return value + 1 end"
        ),
        1
    );
    assert_eq!(
        strict_count(&expression_source("unknown?", "value.field")),
        1
    );
}

#[test]
fn unknown_fields_are_checked_at_their_use() {
    assert_eq!(
        strict_count(
            "---@class Data\n---@field payload unknown\n---@param data Data\nlocal function probe(data) return data.payload() end"
        ),
        1
    );
}

#[test]
fn refinement_does_not_escape_its_branch() {
    assert_eq!(
        strict_count(
            r#"
---@param value unknown
local function probe(value)
    if type(value) == 'number' then local n = value + 1 end
    return value + 1
end
"#
        ),
        1
    );
}

#[test]
fn explicit_cast_allows_operations() {
    assert_eq!(
        strict_count(
            "---@param value unknown\nlocal function probe(value)\n---@cast value number\nreturn value + 1\nend"
        ),
        0
    );
}

#[test]
fn setting_and_suppression_work() {
    let config: Emmyrc = serde_json::from_str(r#"{"strict":{"checkUnknown":true}}"#).unwrap();
    assert!(config.strict.check_unknown);
    assert!(!Emmyrc::default().strict.check_unknown);
    assert_eq!(
        strict_count(
            "---@param value unknown\nlocal function probe(value)\n---@diagnostic disable-next-line: unknown-operation\nreturn value + 1\nend"
        ),
        0
    );
    let mut config = Emmyrc::default();
    config
        .diagnostics
        .enables
        .push(DiagnosticCode::UnknownOperation);
    let diagnostics = check_source_with_emmyrc(&expression_source("unknown", "value + 1"), config);
    let diagnostic = diagnostics
        .iter()
        .find(|d| d.code == DiagnosticCode::UnknownOperation)
        .unwrap();
    assert_eq!(diagnostic.severity, lsp_types::DiagnosticSeverity::ERROR);
}

#[test]
fn meta_sources_remain_exempt() {
    assert_eq!(
        strict_count(&format!(
            "---@meta\n{}",
            expression_source("unknown", "value + 1")
        )),
        0
    );
}

#[test]
fn table_literal_keys_require_known_types() {
    assert_eq!(
        strict_count(&expression_source("unknown", "{[value] = 1}")),
        1
    );
}

#[test]
fn assignment_of_known_value_and_early_exit_guards_narrow() {
    assert_eq!(
        strict_count(
            r#"
---@param value unknown
local function assign(value)
    value = 1
    return value + 1
end
---@param value unknown
local function guard(value)
    if type(value) ~= 'number' then return end
    return value + 1
end
"#
        ),
        0
    );
}

#[test]
fn unknown_class_field_is_not_lost_after_a_branch() {
    assert_eq!(
        strict_count(
            r#"
---@class Packet
---@field value unknown
---@param packet Packet
local function probe(packet)
    if type(packet.value) == 'number' then local n = packet.value + 1 end
    return packet.value + 1
end
"#
        ),
        1
    );
}

#[test]
fn two_unknown_operands_are_reported_individually() {
    assert_eq!(
        strict_count(
            "---@param left unknown\n---@param right unknown\nlocal function probe(left, right) return left + right end"
        ),
        2
    );
}

#[test]
fn field_guard_is_specific_to_the_receiver() {
    assert_eq!(
        strict_count(
            r#"
---@class Message
---@field payload unknown
---@param first Message
---@param second Message
local function probe(first, second)
    if 'number' == type(first.payload) then
        local valid = first.payload + 1
        return second.payload + 1
    end
end
"#
        ),
        1
    );
}

#[test]
fn early_exit_field_guard_narrows_the_surviving_branch() {
    assert_eq!(
        strict_count(
            r#"
---@class Message
---@field payload unknown
---@param message Message
local function probe(message)
    if type(message.payload) ~= 'function' then return end
    return message.payload()
end
"#
        ),
        0
    );
}

#[test]
fn unions_preserve_unknown_and_explicit_any() {
    assert_eq!(
        strict_count(&expression_source("number|unknown", "value + 1")),
        1
    );
    assert_eq!(
        strict_count(&expression_source("any|unknown", "value + 1")),
        0
    );
}

#[test]
fn library_unknown_results_are_checked_in_project_code() {
    use crate::{
        SemanticDatabase, SemanticModel, WorkspaceFolder,
        check::{CheckConfig, check_file},
    };
    use std::{str::FromStr, sync::Arc};
    let mut config = Emmyrc::default();
    config.strict.check_unknown = true;
    let config = Arc::new(config);
    let mut db = SemanticDatabase::new();
    db.update_config(config.clone());
    db.add_main_workspace("C:/ws".into());
    db.add_library_workspace(&WorkspaceFolder::new("C:/sdk".into(), true));
    let library = db.set_file_content(&lsp_types::Uri::from_str("file:///C:/sdk/api.lua").unwrap(),
        Some("---@return unknown\nfunction sdkOpaque() end\n---@param value unknown\nfunction sdkInternal(value) return value.field end".to_string()));
    let main = db.set_file_content(
        &lsp_types::Uri::from_str("file:///C:/ws/main.lua").unwrap(),
        Some("local value = sdkOpaque()\nreturn value.field".to_string()),
    );
    let config = Arc::new(CheckConfig::new(&config));
    assert!(check_file(&SemanticModel::new(&db, library), config.clone()).is_empty());
    assert_eq!(
        count_by_code(
            &check_file(&SemanticModel::new(&db, main), config),
            DiagnosticCode::UnknownOperation
        ),
        1
    );
}

#[test]
fn known_values_survive_loop_and_field_guards() {
    for source in [
        "for i = 1, 4 do local x = i + 1 end",
        "---@class StrictParams\n---@field min number?\n---@param p StrictParams\nlocal function f(p) if p.min ~= nil then return p.min + 1 end end",
        "---@param x number\n---@return number\nlocal function clamp(x) return x end\nlocal t = clamp(1)\nt = t * t * (3 - 2 * t)",
    ] {
        assert_eq!(strict_count(source), 0, "{source}");
    }
}

#[test]
fn optional_fields_and_alias_discriminants_preserve_the_owner() {
    assert_eq!(
        strict_count(
            r#"
---@alias ControlKind 'slider'|'scheme'
---@class Control
---@field kind ControlKind
---@field format string?
---@field min number?
---@field max number?
---@field defaultValue number
---@param value number?
---@param def Control
local function clamp(value, def)
    if value and def.min ~= nil then value = def.min end
    if value and def.max ~= nil then value = def.max end
    return value or def.defaultValue
end
---@param def Control
---@param highlight boolean
local function draw(def, highlight)
    if def.kind ~= 'slider' then return end
    if def.format == nil then return end
    local need = highlight and 1 ~= def.defaultValue
    if need then local ignored = 1 end
    return def.format .. def.defaultValue
end
"#
        ),
        0
    );
}

#[test]
fn typed_tables_support_dynamic_numeric_and_string_keys() {
    assert_eq!(
        strict_count(
            r#"
---@class Cell
---@field value number
---@type table<integer, Cell>
local cells = {}
---@type table<string, Cell>
local names = {}
---@param key string
local function read(key)
    for i = 1, 4 do
        local value = cells[i]
        local result = value.value + names[key].value
    end
end
"#
        ),
        0
    );
}

#[test]
fn mutable_boolean_field_does_not_exclude_its_table() {
    assert_eq!(
        strict_count(
            r#"
local state = { active = false, value = 0 }
---@param active boolean
function setActive(active) state.active = active end
function first() if not state.active then return end return state.value + 1 end
function second() if state.active then return state.value + 1 end end
"#
        ),
        0
    );
}

#[test]
fn library_static_methods_and_generic_union_arguments_keep_returns() {
    use crate::{
        SemanticDatabase, SemanticModel, WorkspaceFolder,
        check::{CheckConfig, check_file},
    };
    use std::{str::FromStr, sync::Arc};
    let mut config = Emmyrc::default();
    config.strict.check_unknown = true;
    let config = Arc::new(config);
    let mut db = SemanticDatabase::new();
    db.update_config(config.clone());
    db.add_main_workspace("C:/ws".into());
    db.add_library_workspace(&WorkspaceFolder::new("C:/sdk".into(), true));
    db.set_file_content(
        &lsp_types::Uri::from_str("file:///C:/sdk/api.lua").unwrap(),
        Some(
            r#"
---@meta
sdk = {}
---@return SdkConfig
function sdk.Config() end
---@class SdkConfig
---@field value number
local config = nil
---@return SdkConfig
function sdk.Config.current() end
---@generic T
---@param x T
---@param low T|number
---@param high T|number
---@return T
function sdk.clamp(x, low, high) end
---@alias SdkMode 1|2
---@param mode SdkMode?
---@return SdkConfig
function sdk.create(mode) end
"#
            .to_string(),
        ),
    );
    let main = db.set_file_content(
        &lsp_types::Uri::from_str("file:///C:/ws/main.lua").unwrap(),
        Some(
            r#"
local cfg = sdk.Config.current()
local value = sdk.clamp(0.5, 0, 1)
---@type SdkConfig?
local created = nil
created = sdk.create(1)
return cfg.value + value + created.value
"#
            .to_string(),
        ),
    );
    let diagnostics = check_file(
        &SemanticModel::new(&db, main),
        Arc::new(CheckConfig::new(&config)),
    );
    assert_eq!(
        count_by_code(&diagnostics, DiagnosticCode::UnknownOperation),
        0
    );
    assert_eq!(count_by_code(&diagnostics, DiagnosticCode::NeedCheckNil), 0);
}

#[test]
fn documented_boolean_member_accepts_new_boolean_values() {
    let diagnostics = check_source(
        r#"
---@class Toggle
---@field enabled boolean
local M = {}
M.enabled = false ---@type boolean
---@param value boolean
function M.set(value) M.enabled = value end
"#,
    );
    assert_eq!(
        count_by_code(&diagnostics, DiagnosticCode::AssignTypeMismatch),
        0
    );
}

#[test]
fn pairs_uses_guarded_optional_table_type() {
    assert_eq!(
        strict_count(
            r#"
---@generic K, V
---@param t table<K, V>
---@return fun(): K, V
function pairs(t) end
---@class Choices
---@field options table<integer, string>?
---@param def Choices
local function render(def)
    if def.options == nil then return end
    for key, label in pairs(def.options) do
        local text = label .. key
    end
end
"#
        ),
        0
    );
}
