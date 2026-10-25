//! Unknown values require narrowing before operations that depend on their type.
use emmylua_parser::{BinaryOperator, LuaAstNode, LuaExpr, LuaIndexKey, UnaryOperator};
use rowan::TextRange;

use super::{CheckContext, Checker};
use crate::semantic_model::{SemanticModel, type_eval::expand_alias_branches};
use crate::{DiagnosticCode, LuaType};

pub struct UnknownOperationChecker;

impl Checker for UnknownOperationChecker {
    const CODES: &[DiagnosticCode] = &[DiagnosticCode::UnknownOperation];

    fn check(context: &mut CheckContext<'_>, model: &SemanticModel<'_>) {
        let Some(tree) = model.syntax_tree() else {
            return;
        };
        // Visit children first to report the source of an unsafe chain only once.
        let expressions: Vec<_> = tree
            .get_red_root()
            .descendants()
            .filter_map(LuaExpr::cast)
            .collect();
        let mut reported = Vec::new();
        for expr in expressions.into_iter().rev() {
            match expr {
                LuaExpr::CallExpr(call) => {
                    if let Some(prefix) = call.get_prefix_expr() {
                        check_operand(context, model, &prefix, "call", &mut reported);
                    }
                }
                LuaExpr::IndexExpr(index) => {
                    if let Some(prefix) = index.get_prefix_expr() {
                        check_operand(context, model, &prefix, "access a field", &mut reported);
                    }
                    if let Some(LuaIndexKey::Expr(key)) = index.get_index_key() {
                        check_operand(context, model, &key, "use as a table key", &mut reported);
                    }
                }
                LuaExpr::TableExpr(table) => {
                    for (_, key) in table.get_fields_with_keys() {
                        if let LuaIndexKey::Expr(key) = key {
                            check_operand(
                                context,
                                model,
                                &key,
                                "use as a table key",
                                &mut reported,
                            );
                        }
                    }
                }
                LuaExpr::BinaryExpr(binary) => {
                    let Some(op) = binary.get_op_token() else {
                        continue;
                    };
                    if matches!(
                        op.get_op(),
                        BinaryOperator::OpEq
                            | BinaryOperator::OpNe
                            | BinaryOperator::OpAnd
                            | BinaryOperator::OpOr
                            | BinaryOperator::OpNilCoalescing
                            | BinaryOperator::OpNop
                    ) {
                        continue;
                    }
                    if let Some((left, right)) = binary.get_exprs() {
                        check_operand(context, model, &left, "apply this operator", &mut reported);
                        check_operand(context, model, &right, "apply this operator", &mut reported);
                    }
                }
                LuaExpr::UnaryExpr(unary) => {
                    let Some(op) = unary.get_op_token() else {
                        continue;
                    };
                    if !matches!(op.get_op(), UnaryOperator::OpNot | UnaryOperator::OpNop)
                        && let Some(operand) = unary.get_expr()
                    {
                        check_operand(
                            context,
                            model,
                            &operand,
                            "apply this operator",
                            &mut reported,
                        );
                    }
                }
                _ => {}
            }
        }
    }
}

fn check_operand(
    context: &mut CheckContext<'_>,
    model: &SemanticModel<'_>,
    expr: &LuaExpr,
    operation: &str,
    reported: &mut Vec<TextRange>,
) {
    // Unresolved names already have undefined-global diagnostics; do not cascade.
    if let LuaExpr::NameExpr(name) = expr
        && model.resolve_name(name.get_position()).is_none()
    {
        return;
    }
    let range = expr.get_range();
    if reported
        .iter()
        .any(|reported| range.contains_range(*reported))
    {
        return;
    }
    let ty = model.type_of_expr_at(expr.get_syntax_id(), range.start());
    let expanded = expand_alias_branches(model, &ty).unwrap_or(ty);
    if !contains_unknown(&expanded) {
        return;
    }
    reported.push(range);
    context.add_diagnostic(
        DiagnosticCode::UnknownOperation,
        range,
        format!("Cannot {operation} on a value of type `unknown`. Narrow its type first."),
    );
}

fn contains_unknown(ty: &LuaType) -> bool {
    match ty {
        LuaType::Unknown => true,
        LuaType::Union(union) => {
            // An explicit any remains an opt-out from checking.
            !union.iter().any(|ty| matches!(ty, LuaType::Any)) && union.iter().any(contains_unknown)
        }
        LuaType::Intersection(intersection) => {
            intersection.get_types().iter().all(contains_unknown)
        }
        _ => false,
    }
}
