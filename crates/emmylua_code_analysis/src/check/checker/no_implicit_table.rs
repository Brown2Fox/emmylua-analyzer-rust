use super::{CheckContext, Checker};
use crate::{DeclKind, DiagnosticCode, SemanticModel};
use emmylua_parser::{LuaAstNode, LuaExpr};

pub struct NoImplicitTableChecker;
impl Checker for NoImplicitTableChecker {
    const CODES: &[DiagnosticCode] = &[DiagnosticCode::NoImplicitTable];
    fn check(context: &mut CheckContext<'_>, model: &SemanticModel<'_>) {
        let Some(facts) = model.file_facts() else {
            return;
        };
        let Some(tree) = model.syntax_tree() else {
            return;
        };
        let root = tree.get_red_root();
        for decl in &facts.decls {
            if !matches!(decl.kind, DeclKind::Local { .. }) || decl.doc_type_syntax.is_some() {
                continue;
            }
            if decl
                .owner_syntax
                .and_then(|owner| facts.type_def_by_owner_syntax(owner))
                .is_some()
            {
                continue;
            }
            let Some(mut expr) = decl
                .value_expr_syntax
                .and_then(|id| id.to_node_from_root(&root))
                .and_then(LuaExpr::cast)
            else {
                continue;
            };
            while let LuaExpr::ParenExpr(paren) = &expr {
                let Some(inner) = paren.get_expr() else {
                    break;
                };
                expr = inner;
            }
            if matches!(expr, LuaExpr::TableExpr(_)) {
                context.add_diagnostic(DiagnosticCode::NoImplicitTable, decl.name_range,
                    format!("Table `{}` needs an explicit type. Add a type annotation or a class declaration.", decl.name));
            }
        }
    }
}
