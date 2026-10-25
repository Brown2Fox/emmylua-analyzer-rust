//! Report missing parameter types without requiring redundant documentation.
use emmylua_parser::{LuaAstNode, LuaClosureExpr};

use super::{CheckContext, Checker};
use crate::semantic_model::{SemanticModel, infer::vm};
use crate::{DiagnosticCode, LuaType};

pub struct NoImplicitAnyChecker;

impl Checker for NoImplicitAnyChecker {
    const CODES: &[DiagnosticCode] = &[DiagnosticCode::NoImplicitAny];

    fn check(context: &mut CheckContext<'_>, model: &SemanticModel<'_>) {
        let Some(facts) = model.file_facts() else {
            return;
        };
        let Some(tree) = model.syntax_tree() else {
            return;
        };
        let root = tree.get_red_root();
        for signature in &facts.signatures {
            let Some(closure) = signature
                .closure_syntax
                .to_node_from_root(&root)
                .and_then(LuaClosureExpr::cast)
            else {
                continue;
            };
            let Some(params) = closure.get_params_list() else {
                continue;
            };
            for (index, param) in params.get_params().enumerate() {
                let name = param
                    .get_name_token()
                    .map(|token| token.get_name_text().to_string())
                    .unwrap_or_else(|| "...".to_string());
                // Explicit unknown/any is an intentional boundary, not an inference failure.
                if signature.docs.as_ref().is_some_and(|docs| {
                    docs.param_types.iter().any(|(documented, _)| {
                        documented == &name || (param.is_dots() && documented == "...")
                    })
                }) {
                    continue;
                }
                if model
                    .expected_variable_param_for_closure(signature.closure_syntax, index)
                    .is_some()
                    || model
                        .expected_member_param_for_closure(signature.closure_syntax, index)
                        .is_some()
                    || vm::closure_param_binding(model, signature.closure_syntax, index).is_some()
                {
                    continue;
                }
                let inferred = model
                    .decl_by_offset(param.get_range().start())
                    .and_then(|decl| model.type_of_decl(&decl));
                if inferred.is_some_and(|ty| !matches!(ty, LuaType::Any | LuaType::Unknown)) {
                    continue;
                }
                context.add_diagnostic(DiagnosticCode::NoImplicitAny, param.get_range(),
                    format!("Parameter `{name}` has no declared or inferred type. Add a @param annotation or a typed function signature."));
            }
        }
    }
}
