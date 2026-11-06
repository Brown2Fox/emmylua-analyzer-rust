use super::*;
use emmylua_code_analysis::{CheckConfig, EmmyLuaAnalysis, Emmyrc};
use emmylua_parser::LuaLanguageLevel;
use std::sync::Arc;

fn compile(source: &str) -> Output {
    transpile(source, LuaLanguageLevel::Lua51).unwrap()
}

fn diagnostics(source: &str, library: &str) -> Vec<lsp_types::Diagnostic> {
    let mut config = Emmyrc::default();
    config.strict.check_unknown = true;
    config.strict.no_implicit_any = true;
    let config = Arc::new(config);
    let mut analysis = EmmyLuaAnalysis::new();
    analysis.update_config(config.clone());
    let root = std::env::current_dir().unwrap();
    analysis.update_file_by_path(&root.join("sdk.d.lua"), Some(library.into()));
    let output = compile(source);
    let id = analysis
        .update_file_by_path(&root.join("input.cua"), Some(output.analysis_lua))
        .unwrap();
    analysis
        .diagnose_file_with_config(id, Arc::new(CheckConfig::new(&config)))
        .unwrap()
        .into_iter()
        .filter(|d| {
            matches!(
                d.severity,
                Some(lsp_types::DiagnosticSeverity::ERROR | lsp_types::DiagnosticSeverity::WARNING)
            )
        })
        .collect()
}

#[test]
fn erases_types_without_changing_runtime_lines_or_expressions() {
    let source = "local value: number = 42\r\nlocal function add(a: number, b: number): number\r\n return a + b\r\nend\r\nreturn add(value, 1)\r\n";
    let output = compile(source);
    assert_eq!(output.lua.len(), source.len());
    assert_eq!(
        output.lua.matches('\n').count(),
        source.matches('\n').count()
    );
    assert!(output.lua.contains("return a + b"));
    assert!(output.analysis_lua.contains("---@param a number"));
    assert!(output.analysis_lua.contains("---@return number"));
    assert!(
        diagnostics(source, "").is_empty(),
        "{:#?}",
        diagnostics(source, "")
    );
}

#[test]
fn literals_comments_methods_and_plain_lua_are_untouched() {
    let source = "-- local x: Broken = 0\nlocal s = 'function f(x: fake): fake'\nlocal long = [=[local x: fake]=]\nlocal t = {}\nfunction t:method(x) return x end\nreturn t:method(s)\n";
    let output = compile(source);
    assert_eq!(output.lua, source);
    assert_eq!(output.analysis_lua, source);
}

#[test]
fn trailing_annotation_belongs_to_previous_statement() {
    let output =
        compile("local first = 1 ---@type number\nlocal second: string = 'ok'\nreturn second");
    assert!(output.analysis_lua.contains("---@type string"));
}

#[test]
fn casts_assert_expression_types_without_runtime_conversion() {
    for source in [
        "local function f(x: unknown): string return (x as string) end",
        "local function f(x: unknown): number return (x as { value: number }).value end",
        "local function f(x: unknown): number return (x as fun(): number)() end",
        "local function f(x: unknown): number return ((x as string) as number) end",
        "local function f(x: number, y: number): string return (x + y as string) end",
        "local function f(x: unknown): string return ('x' .. (x as string)) end",
        "local function f(x: unknown): string local t: table<string, string> = (x as table<string, string>); return t.key end",
        "local function f(x: unknown): string local s: string = (x as ']]'); return s end",
        "local function f(x: unknown): string? return (x as string|nil) end",
        "local function f(): string return ([[test]] as string) end",
        "local function f(): number return (function() return 1 end as fun(): number)() end",
        "local function f(x: unknown): string return (\n x -- value\n as string) end",
    ] {
        let output = compile(source);
        assert_eq!(output.lua.len(), source.len());
        assert!(!output.lua.contains(" as "), "{}", output.lua);
        assert!(
            diagnostics(source, "").is_empty(),
            "{source}: {:#?}",
            diagnostics(source, "")
        );
    }
    let bad = "local function f(x: unknown): number return (x as string) end";
    assert!(!diagnostics(bad, "").is_empty());
    let runtime = compile("return (call() as string)");
    assert_eq!(runtime.lua, "return (call()          )");
}

#[test]
fn casts_reject_ambiguous_continuations_and_preserve_as_identifiers() {
    for source in [
        "return (x as)",
        "return (x as number|)",
        "return x as number and y",
        "return x as number()",
        "return (x as number as string)",
    ] {
        assert!(
            transpile(source, LuaLanguageLevel::Lua51).is_err(),
            "{source}"
        );
    }
    let plain =
        "local as = 1; local t = {as = as}; function t.as(as) return as end; return t.as(as)";
    assert_eq!(compile(plain).lua, plain);
    let calls = "local as = function() end\nas() as()\nas = function() end\nas()";
    assert_eq!(compile(calls).lua, calls);
}

#[test]
fn checks_sdk_types_and_reports_bad_assignments_calls_returns_and_unknown() {
    let library = "---@class Camera\n---@field zoom number\nlocal Camera = {}\n---@return Camera\nfunction getCamera() end\n";
    let good = "local camera: Camera = getCamera()\nlocal function zoom(c: Camera): number return c.zoom end\nreturn zoom(camera)";
    assert!(
        diagnostics(good, library).is_empty(),
        "{:#?}",
        diagnostics(good, library)
    );
    for bad in [
        "local n: number = 'wrong'",
        "local function f(n: number): number return n end\nf('wrong')",
        "local function f(): number return 'wrong' end",
        "local function f(n: unknown): number return n end",
    ] {
        assert!(!diagnostics(bad, "").is_empty(), "{bad}");
    }
}

#[test]
fn supports_complex_types_multiple_locals_varargs_and_returns() {
    let source = "local a: number, b: string = 1, 'x'\nlocal tableValue: table<string, table<string, number?>> = {}\nlocal callback: fun(x: number): string\nlocal function f(...: number): (number, string) return a, b end";
    let output = compile(source);
    assert!(output.analysis_lua.contains("---@type number, string"));
    assert!(output.analysis_lua.contains("---@return number, string"));
    assert!(output.analysis_lua.contains("---@param ... number"));
    assert!(
        diagnostics(source, "").is_empty(),
        "{:#?}",
        diagnostics(source, "")
    );
}

#[test]
fn annotations_can_describe_types_used_by_inline_syntax() {
    let source = "---@alias Count number\nlocal n: Count = 1\n---@param y string\nlocal function f(x: Count, y): Count return x end\nreturn f(n, 'x')";
    assert!(
        diagnostics(source, "").is_empty(),
        "{:#?}",
        diagnostics(source, "")
    );
}

#[test]
fn rejects_malformed_types_and_duplicate_contracts() {
    for source in [
        "local n: = 1",
        "local n: number| = 1",
        "local n: table<string,> = {}",
        "local function f(x: number end",
        "local a: number, b = 1, 2",
        "---@type string\nlocal n: number = 1",
        "--[[@type string]]\nlocal n: number = 1",
        "---@param x string\nlocal function f(x: number) end",
        "---@return string\nlocal function f(): number return 1 end",
    ] {
        assert!(
            transpile(source, LuaLanguageLevel::Lua51).is_err(),
            "{source}"
        );
    }
}

#[test]
fn maps_nested_function_diagnostics_to_original_offsets() {
    let source = "local function outer(x: number): number\n local function inner(y: number): number return y end\n return inner('ошибка')\nend";
    let output = compile(source);
    let generated = output.analysis_lua.find("'ошибка'").unwrap();
    assert_eq!(
        output.original_offset(generated),
        source.find("'ошибка'").unwrap()
    );
    assert!(!diagnostics(source, "").is_empty());
}

#[test]
fn rejects_unsupported_anonymous_types_and_checks_named_methods() {
    let source = "local f = function(x: number): number return x end\nf('bad')";
    assert!(transpile(source, LuaLanguageLevel::Lua51).is_err());
    let method =
        "local t = {}\nfunction t:zoom(value: number): number return value end\nt:zoom('bad')";
    assert!(
        diagnostics(method, "").iter().any(|d| d.code
            == Some(lsp_types::NumberOrString::String(
                "param-type-mismatch".into()
            ))),
        "{:#?}",
        diagnostics(method, "")
    );
}

#[test]
fn unparenthesized_casts_cover_complete_expressions() {
    for source in [
        "local function f(x: unknown): string local s = x as string; return s end",
        "local function f(x: unknown): string return x as string end",
        "local function f(x: number, y: number): string return x + y as string end",
        "local function f(x: unknown): string local t = {value = x as string}; return t.value end",
        "local function take(x: string): string return x end\nlocal function f(x: unknown): string return take(x as string) end",
        "local function f(x: unknown): (string, number) return x as string, x as number end",
        "local function f(x: unknown): string local s = x as string\nreturn s end",
    ] {
        assert!(
            diagnostics(source, "").is_empty(),
            "{source}: {:#?}",
            diagnostics(source, "")
        );
    }
    assert!(
        !diagnostics(
            "local function f(x: unknown): number return x as string end",
            ""
        )
        .is_empty()
    );
    assert_eq!(
        compile("local label = options[index] as string").lua,
        "local label = options[index]          "
    );
}

#[test]
fn class_and_alias_declarations_are_erased_and_checked() {
    let source = "alias Kind = 'slider' | 'cbox'\nclass ParamDef\n kind: Kind\n displayName: string\n options?: string[]\nend\nlocal value: ParamDef = {kind = 'slider', displayName = 'Distance'}\nreturn value";
    let output = compile(source);
    assert_eq!(output.lua.len(), source.len());
    assert!(!output.lua.contains("class"));
    assert!(output.analysis_lua.contains("---@class ParamDef"));
    assert!(output.analysis_lua.contains("---@field options? string[]"));
    assert!(
        diagnostics(source, "").is_empty(),
        "{:?}",
        diagnostics(source, "")
    );
    let invalid = source.replace("displayName = 'Distance'", "displayName = 12");
    assert!(!diagnostics(&invalid, "").is_empty());
    for name in ["ParamDef", "Kind", "displayName", "string"] {
        let offset = source.find(name).unwrap();
        assert_eq!(
            output.original_offset(output.generated_offset(offset)),
            offset
        );
    }
}

#[test]
fn class_headers_fields_and_invalid_declarations() {
    let source = "class Base\n id: integer\nend\nclass (exact) Box<T>: Base\n value: T\n [string]: any\nend\nalias Items<T> = table<string, T>\nlocal unrelated = {other = 1}\nreturn unrelated";
    let output = compile(source);
    assert!(
        output
            .analysis_lua
            .contains("---@class (exact) Box<T>: Base")
    );
    assert!(output.analysis_lua.contains("---@alias Items<T>"));
    assert!(
        diagnostics(source, "").is_empty(),
        "{:?}",
        diagnostics(source, "")
    );
    for source in [
        "class Broken\n value: number",
        "class Broken\n value number\nend",
        "alias Broken number",
        "alias Broken =",
        "class Broken\n value:\nend",
    ] {
        assert!(
            transpile(source, LuaLanguageLevel::Lua51).is_err(),
            "{source}"
        );
    }
    let plain = "class()\nalias({})\nlocal class = 1\nlocal alias = 2\nreturn class + alias";
    assert_eq!(compile(plain).lua, plain);
}

#[test]
fn native_class_binds_its_explicit_runtime_implementation() {
    let source = "class Module\nend\nlocal M: Module = {}\nfunction M.get(): number return 42 end\nlocal function use(value: Module): number return value.get() end\nreturn use(M)";
    assert!(
        diagnostics(source, "").is_empty(),
        "{:?}",
        diagnostics(source, "")
    );
    let source = "class (exact) Record\n name: string\nend\nlocal unrelated: table<string, number> = {other = 1}\nreturn unrelated";
    assert!(
        diagnostics(source, "").is_empty(),
        "{:?}",
        diagnostics(source, "")
    );
}

#[test]
fn unparenthesized_return_lists_match_parenthesized_lists() {
    for types in [
        "number, number",
        "number, string?",
        "table<string, number>, boolean",
    ] {
        let body = if types.starts_with("table") {
            "return {}, true"
        } else if types.contains("string") {
            "return 1, nil"
        } else {
            "return 1, 2"
        };
        let plain = format!("local function values(): {types}\n {body}\nend\nreturn values()");
        let grouped = format!("local function values(): ({types})\n {body}\nend\nreturn values()");
        for source in [plain, grouped] {
            let output = compile(&source);
            assert!(output.analysis_lua.contains(&format!("---@return {types}")));
            assert!(output.lua.contains(body));
            assert_eq!(output.lua.len(), source.len());
            assert!(
                diagnostics(&source, "").is_empty(),
                "{:?}",
                diagnostics(&source, "")
            );
            let start = source.find(types).unwrap();
            for offset in start..start + types.len() {
                assert_eq!(
                    output.original_offset(output.generated_offset(offset)),
                    offset
                );
            }
        }
    }
    let source = "local function values(): number, number return 1, 'bad' end";
    assert!(!diagnostics(source, "").is_empty());
    assert!(transpile("local function f(): number,\nend", LuaLanguageLevel::Lua51).is_err());
}
