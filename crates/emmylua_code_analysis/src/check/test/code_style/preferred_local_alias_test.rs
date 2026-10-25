#[cfg(test)]
mod test {
    use crate::DiagnosticCode;
    use crate::VirtualWorkspace;

    #[test]
    fn test_feat_724() {
        let mut ws = VirtualWorkspace::new_with_init_std_lib();

        assert!(!ws.has_no_diagnostic(
            DiagnosticCode::PreferredLocalAlias,
            r#"
            local gsub = string.gsub
            print(string.gsub("hello", "l", "0"))
            "#,
        ));

        assert!(ws.has_no_diagnostic(
            DiagnosticCode::PreferredLocalAlias,
            r#"
            local t = {
                a = ""
            }
            local h = t.a
            t.a = 'h'
            print(t.a)
            "#,
        ));
    }
}

#[cfg(test)]
mod mutation_regressions {
    use crate::{DiagnosticCode, VirtualWorkspace};

    fn check(source: &str, expected: bool) {
        let mut ws = VirtualWorkspace::new_with_init_std_lib();
        assert_eq!(
            !ws.has_no_diagnostic(DiagnosticCode::PreferredLocalAlias, source),
            expected
        );
    }

    #[test]
    fn refresh_of_cached_value_is_not_self_assignment() {
        check(
            "local t = { value = 1 }; local cached = t.value; cached = t.value",
            false,
        );
    }

    #[test]
    fn reassigned_alias_is_not_equivalent_to_field() {
        check(
            "local t = { value = 1 }; local cached = t.value; cached = 2; print(t.value)",
            false,
        );
    }

    #[test]
    fn closure_writes_are_not_ordered_by_source_position() {
        for source in [
            "local t = { value = 1 }; local cached = t.value; local function read() return t.value end; local function update() cached = 2 end",
            "local t = { value = 1 }; local cached = t.value; local function read() return t.value end; local function update() t.value = 2 end",
            "local t = { value = 1 }; local cached = t.value; local function update() cached = 2 end; local function read() return t.value end",
        ] {
            check(source, false);
        }
    }

    #[test]
    fn conditional_write_invalidates_alias() {
        check(
            "local t = { value = 1 }; local cached = t.value; if condition then cached = 2 end; print(t.value)",
            false,
        );
    }

    #[test]
    fn replacing_source_table_invalidates_alias() {
        check(
            "local t = { value = 1 }; local cached = t.value; t = { value = 2 }; print(t.value)",
            false,
        );
    }

    #[test]
    fn bracket_and_dynamic_writes_invalidate_alias() {
        check(
            "local t = { value = 1 }; local cached = t.value; t['value'] = 2; print(t.value)",
            false,
        );
        check(
            "local t = { value = 1 }; local cached = t.value; t[key] = 2; print(t.value)",
            false,
        );
    }

    #[test]
    fn shadowed_alias_is_not_suggested() {
        check(
            "local t = { value = 1 }; local cached = t.value; do local cached = 2; print(t.value) end",
            false,
        );
    }

    #[test]
    fn unrelated_writes_preserve_valid_alias() {
        check(
            "local t = { value = 1, other = 0 }; local cached = t.value; t.other = 2; do local cached = 2; cached = 3 end; print(t.value)",
            true,
        );
    }

    #[test]
    fn stable_alias_remains_available_inside_closure() {
        check(
            "local gsub = string.gsub; local function replace(s) return string.gsub(s, 'a', 'b') end",
            true,
        );
    }
}
