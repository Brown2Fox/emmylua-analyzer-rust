use super::CheckContext;
use crate::VariadicType;
use crate::semantic_model::{
    SemanticModel, render::humanize_type, type_check::rejects_unknown_conversion,
};
use crate::{DiagnosticCode, LuaType};
use emmylua_parser::LuaAstNode;
use rowan::TextRange;

pub(super) fn report(
    context: &mut CheckContext<'_>,
    model: &SemanticModel<'_>,
    source: &LuaType,
    target: &LuaType,
    range: TextRange,
) -> bool {
    if !rejects_unknown_conversion(model, source, target) {
        return false;
    }
    context.add_diagnostic(
        DiagnosticCode::UnknownConversion,
        range,
        message(model, source, target),
    );
    true
}

pub(super) fn message(model: &SemanticModel<'_>, source: &LuaType, target: &LuaType) -> String {
    format!(
        "Cannot implicitly convert `{}` to `{}`. Narrow the unknown value first.",
        humanize_type(model, source),
        humanize_type(model, target)
    )
}

// Conversion diagnostics need the narrowed type at each expression.
pub(super) fn expression_types(
    model: &SemanticModel,
    exprs: &[emmylua_parser::LuaExpr],
    var_count: Option<usize>,
) -> Vec<(LuaType, TextRange)> {
    let mut value_types: Vec<(LuaType, TextRange)> = Vec::new();
    for (idx, expr) in exprs.iter().enumerate() {
        if let Some(var_count) = var_count
            && value_types.len() >= var_count
        {
            break;
        }

        let mut expr_type = model.type_of_expr_at(expr.get_syntax_id(), expr.get_position());
        if idx + 1 < exprs.len() {
            expr_type = expr_type.get_result_slot_type(0).unwrap_or(LuaType::Nil);
        }
        if let Some(var_count) = var_count
            && expr_type.contain_multi_return()
        {
            if idx < var_count {
                for i in idx..var_count {
                    if let Some(typ) = expr_type.get_result_slot_type(i - idx) {
                        value_types.push((typ, expr.get_range()));
                    } else {
                        break;
                    }
                }
            }
            break;
        }

        match &expr_type {
            LuaType::Variadic(variadic) => {
                match variadic.as_ref() {
                    VariadicType::Base(base) | VariadicType::Repeat(base) => {
                        value_types.push((base.clone(), expr.get_range()));
                    }
                    VariadicType::Multi(types) => {
                        for typ in types {
                            value_types.push((typ.clone(), expr.get_range()));
                        }
                    }
                }
                break;
            }
            _ => value_types.push((expr_type, expr.get_range())),
        }
    }
    value_types
}
