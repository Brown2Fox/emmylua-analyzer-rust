use super::{check_source_with_emmyrc, count_by_code};
use crate::{DiagnosticCode, Emmyrc};

#[test]
fn documented_name_overloads_accept_arguments_but_not_extra_arguments() {
    let mut ws = crate::VirtualWorkspace::new();
    ws.def_file(
        "api.lua",
        r#"
---@class Extension
local Extension = {}
---@overload fun(extension: string, mode: 'manual'|'auto')
---@param extension Extension
---@param mode 'manual'|'auto'
function unloadMode(extension, mode) end
"#,
    );
    assert!(ws.has_no_diagnostic(
        DiagnosticCode::ParamTypeMismatch,
        "unloadMode('camera', 'manual')"
    ));
    assert!(!ws.has_no_diagnostic(
        DiagnosticCode::ParamTypeMismatch,
        "unloadMode(42, 'manual')"
    ));
}

fn count(source: &str, strict: bool) -> usize {
    let mut config = Emmyrc::default();
    config.strict.check_unknown = strict;
    count_by_code(
        &check_source_with_emmyrc(source, config),
        DiagnosticCode::UnknownConversion,
    )
}

#[test]
fn assignments_arguments_and_returns_reject_unknown() {
    for body in [
        "---@type number\nlocal n = value",
        "---@type number\nlocal n = 0\nn = value",
        "---@class Holder\n---@field n number\n---@type Holder\nlocal obj = {}\nobj.n = value",
        "---@param n number\nlocal function take(n) end\ntake(value)",
        "---@return number\nlocal function give() return value end",
        "---@type {n: number}\nlocal obj = {n = value}",
    ] {
        let source = format!("---@type unknown\nlocal value\n{body}");
        assert_eq!(count(&source, true), 1, "{body}");
        assert_eq!(count(&source, false), 0, "{body}");
    }
}

#[test]
fn any_and_unknown_targets_are_allowed() {
    for target in ["any", "unknown", "number|unknown", "number|any"] {
        let source =
            format!("---@type unknown\nlocal value\n---@type {target}\nlocal result = value");
        assert_eq!(count(&source, true), 0, "{target}");
    }
    assert_eq!(
        count(
            "---@type any\nlocal value\n---@type number\nlocal result = value",
            true
        ),
        0
    );
}

#[test]
fn aliases_and_nullable_unions_keep_unknown() {
    for source_type in ["unknown", "unknown?", "number|unknown", "Opaque"] {
        let source = format!(
            "---@alias Opaque unknown\n---@type {source_type}\nlocal value\n---@type number?\nlocal n = value"
        );
        assert_eq!(count(&source, true), 1, "{source_type}");
    }
}

#[test]
fn narrowed_values_can_cross_typed_boundaries() {
    assert_eq!(
        count(
            r#"
---@param value unknown
---@return number
local function convert(value)
    if type(value) ~= 'number' then return 0 end
    ---@type number
    local result = value
    return result
end
"#,
            true
        ),
        0
    );
}

#[test]
fn return_slots_are_checked_individually() {
    assert_eq!(
        count(
            r#"
---@param value unknown
---@return string, number
local function f(value) return 'ok', value end
"#,
            true
        ),
        1
    );
    assert_eq!(
        count(
            r#"
---@param value unknown
---@return unknown, number
local function f(value) return value, 1 end
"#,
            true
        ),
        0
    );
}

#[test]
fn varargs_and_overloads_respect_unknown() {
    assert_eq!(
        count(
            r#"
---@param ... number
local function take(...) end
---@type unknown
local value
take(1, value)
"#,
            true
        ),
        1
    );
    assert_eq!(
        count(
            r#"
---@param value number
---@overload fun(value: unknown)
local function take(value) end
---@type unknown
local value
take(value)
"#,
            true
        ),
        0
    );
}

#[test]
fn nested_unknown_cannot_be_laundered_through_a_container() {
    assert_eq!(
        count(
            r#"
---@type {value: unknown}
local source
---@type {value: number}
local target = source
"#,
            true
        ),
        1
    );
}

#[test]
fn multiple_assignment_slots_are_checked() {
    for assignment in [
        "---@type string, number\nlocal a, b = values()",
        "---@type string\nlocal a = ''\n---@type number\nlocal b = 0\na, b = values()",
    ] {
        let source =
            format!("---@return string, unknown\nlocal function values() end\n{assignment}");
        assert_eq!(count(&source, true), 1, "{assignment}");
    }
}

#[test]
fn forwarded_returns_keep_unknown() {
    assert_eq!(
        count(
            "---@return string, unknown\nlocal function source() end\n---@return string, number\nlocal function target() return source() end",
            true
        ),
        1
    );
}

#[test]
fn diagnostic_configuration_and_suppression() {
    let source = "---@type unknown\nlocal source\n---@type number\nlocal target = source";
    let mut config = Emmyrc::default();
    config
        .diagnostics
        .enables
        .push(DiagnosticCode::UnknownConversion);
    assert_eq!(
        count_by_code(
            &check_source_with_emmyrc(source, config),
            DiagnosticCode::UnknownConversion
        ),
        1
    );
    assert_eq!(
        count(
            &format!("---@diagnostic disable: unknown-conversion\n{source}"),
            true
        ),
        0
    );
}

#[test]
fn multiple_call_arguments_are_checked() {
    assert_eq!(
        count(
            "---@return string, unknown\nlocal function source() end\n---@param a string\n---@param b number\nlocal function target(a, b) end\ntarget(source())",
            true
        ),
        1
    );
}

#[test]
fn containers_and_function_variance() {
    for (source, target, errors) in [
        ("unknown[]", "number[]", 1),
        ("table<string, unknown>", "table<string, number>", 1),
        ("fun(): unknown", "fun(): number", 1),
        ("fun(value: number)", "fun(value: unknown)", 1),
        ("fun(value: unknown)", "fun(value: number)", 0),
    ] {
        let text =
            format!("---@type {source}\nlocal source\n---@type {target}\nlocal target = source");
        assert_eq!(count(&text, true), errors, "{source} -> {target}");
    }
}

#[test]
fn table_argument_overloads() {
    for (overload, expected) in [("", 1), ("---@overload fun(value: unknown)\n", 0)] {
        let source = format!(
            "---@type unknown\nlocal value\n---@param value {{n: number}}\n{overload}local function take(value) end\ntake({{n = value}})"
        );
        assert_eq!(count(&source, true), expected, "{overload}");
    }
}

#[test]
fn expanded_varargs_cannot_hide_unknown() {
    assert_eq!(
        count(
            "---@return number, unknown\nlocal function source() end\n---@param ... number\nlocal function target(...) end\ntarget(source())",
            true
        ),
        1
    );
}

#[test]
fn class_table_argument_reports_once() {
    assert_eq!(
        count(
            "---@class Holder\n---@field n number\nlocal Holder = {}\n---@type unknown\nlocal value\n---@param holder Holder\nlocal function take(holder) end\ntake({n=value})",
            true
        ),
        1
    );
}

#[test]
fn known_sdk_shapes_survive_guards() {
    assert_eq!(
        count(
            r#"
---@class Car
---@field inPit boolean
local Car = {}
local ac = {}
---@return Car
function ac.getCar() end
---@return number
function ac.axis() end
local wasInPit = false
local function update()
    local car = ac.getCar()
    if car == nil then return end
    local inPit = car.inPit
    if inPit and not wasInPit then print('pit') end
    wasInPit = inPit
end
---@return number
local function read()
    if type(ac.axis) ~= 'function' then return 0 end
    return ac.axis() or 0
end
"#,
            true
        ),
        0
    );
}

#[test]
fn invalid_arguments_do_not_erase_concrete_return_contracts() {
    let source = r#"
---@param value number
---@return number, number
local function pair(value) return value, value end
---@type number, number
local first, second = pair('invalid')
"#;
    let mut config = Emmyrc::default();
    config.strict.check_unknown = true;
    let diagnostics = check_source_with_emmyrc(source, config);
    assert_eq!(
        count_by_code(&diagnostics, DiagnosticCode::UnknownConversion),
        0
    );
    assert_eq!(
        count_by_code(&diagnostics, DiagnosticCode::ParamTypeMismatch),
        1
    );
}
