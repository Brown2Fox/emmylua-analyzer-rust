use super::{check_source, check_source_with_emmyrc, count_by_code};
use crate::{DiagnosticCode, Emmyrc};
fn count(source: &str) -> usize {
    let mut config = Emmyrc::default();
    config.strict.no_implicit_table = true;
    count_by_code(
        &check_source_with_emmyrc(source, config),
        DiagnosticCode::NoImplicitTable,
    )
}
#[test]
fn explicit_types_for_local_table_literals() {
    assert_eq!(
        count("local value = { active = false }; value.active = true"),
        1
    );
    assert_eq!(count("local a, b = {}, ({})"), 2);
    assert_eq!(
        count("---@type table<string, number>\nlocal values = {}"),
        0
    );
    assert_eq!(
        count("---@class Values\n---@field active boolean\nlocal values = {active = false}"),
        0
    );
    assert_eq!(count("---@type any\nlocal values = {}"), 0);
    assert_eq!(
        count("local value = 1; local function f() return {} end"),
        0
    );
    assert_eq!(
        count("---@type table<string, table<string, number>>\nlocal values = {one = {two = 2}}"),
        0
    );
}
#[test]
fn optional_and_suppressible() {
    assert_eq!(
        count_by_code(
            &check_source("local value = {}"),
            DiagnosticCode::NoImplicitTable
        ),
        0
    );
    assert_eq!(
        count("---@diagnostic disable: no-implicit-table\nlocal value = {}"),
        0
    );
    let config: Emmyrc = serde_json::from_str(r#"{"strict":{"noImplicitTable":true}}"#).unwrap();
    assert!(config.strict.no_implicit_table);
}
