//! preferred_local_alias — when `local alias = a.b.c` exists, suggests using the alias for later `a.b.c` references.
//!
//! Only suggest aliases without known writes in this file. Writes in closures and
//! branches are collected before checking: source order does not imply execution order.

use std::collections::{HashMap, HashSet};

use emmylua_parser::{
    LuaAssignStat, LuaAst, LuaAstNode, LuaExpr, LuaIndexExpr, LuaIndexKey, LuaLocalStat,
    LuaSyntaxKind, LuaVarExpr, PathTrait,
};
use rowan::TextRange;

use crate::DiagnosticCode;
use crate::LuaType;
use crate::semantic_db::def::SemanticId;
use crate::semantic_model::SemanticModel;

use crate::check::checker::{CheckContext, Checker};

pub struct PreferredLocalAliasChecker;

impl Checker for PreferredLocalAliasChecker {
    const CODES: &[DiagnosticCode] = &[DiagnosticCode::PreferredLocalAlias];

    fn check(context: &mut CheckContext<'_>, semantic_model: &SemanticModel<'_>) {
        let Some(tree) = semantic_model.syntax_tree() else {
            return;
        };
        let mut alias_set = LocalAliasSet::new();
        let chunk = tree.get_chunk_node();
        let mut writes = AliasWrites::default();
        for assign in chunk.descendants::<LuaAssignStat>() {
            writes.collect(semantic_model, &assign);
        }
        for walk in chunk.walk_descendants::<LuaAst>() {
            match walk {
                rowan::WalkEvent::Enter(node) => {
                    if is_scope(&node) {
                        alias_set.push();
                    }
                    match node {
                        LuaAst::LuaLocalStat(local_stat) => {
                            collect_local_alias(
                                &mut alias_set,
                                semantic_model,
                                &local_stat,
                                &writes,
                            );
                        }
                        LuaAst::LuaIndexExpr(index_expr) => {
                            check_index_expr_preference(
                                context,
                                &alias_set,
                                semantic_model,
                                &index_expr,
                            );
                        }
                        _ => {}
                    }
                }
                rowan::WalkEvent::Leave(node) => {
                    if is_scope(&node) {
                        alias_set.pop();
                    }
                }
            }
        }
    }
}

fn is_scope(node: &LuaAst) -> bool {
    matches!(
        node.syntax().kind().into(),
        LuaSyntaxKind::Chunk | LuaSyntaxKind::Block | LuaSyntaxKind::ClosureExpr
    )
}

fn collect_local_alias(
    alias_set: &mut LocalAliasSet,
    semantic_model: &SemanticModel<'_>,
    local_stat: &LuaLocalStat,
    writes: &AliasWrites,
) {
    let local_list = local_stat.get_local_name_list().collect::<Vec<_>>();
    let value_exprs = local_stat.get_value_exprs().collect::<Vec<_>>();
    let min_len = local_list.len().min(value_exprs.len());
    for i in 0..min_len {
        let local_name = &local_list[i];
        let value_expr = &value_exprs[i];
        let LuaExpr::IndexExpr(index_expr) = value_expr else {
            continue;
        };
        if !is_only_dot_index_expr(value_expr).unwrap_or(false) {
            continue;
        }
        // M0: only single-level member aliases (`local alias = t.a`); chained (`a.b.c`) is left to chained member resolution.
        let Some(LuaExpr::NameExpr(_)) = index_expr.get_prefix_expr() else {
            continue;
        };
        let Some(access_path) = index_expr.get_access_path() else {
            continue;
        };
        // Root variable declaration + outermost field member declaration (resolve_member: the only member-resolution entry point).
        let Some(ref_var) = find_ref_var_decl_id(semantic_model, value_expr) else {
            continue;
        };
        let Some(resolved) = semantic_model.resolve_member(index_expr) else {
            continue;
        };
        let ref_field = resolved
            .member_id
            .or_else(|| runtime_member_id(semantic_model, index_expr, &resolved.name));
        let Some(name_token) = local_name.get_name_token() else {
            continue;
        };
        let Some(alias_decl) = semantic_model.decl_by_offset(local_name.get_position()) else {
            continue;
        };
        if writes.variables.contains(&alias_decl)
            || writes.variables.contains(&ref_var)
            || writes.dynamic_roots.contains(&ref_var)
            || writes
                .paths
                .contains(&(ref_var.clone(), resolved.name.to_string()))
            || ref_field
                .as_ref()
                .is_some_and(|field| writes.fields.contains(field))
        {
            continue;
        }
        let preferred_name = name_token.get_name_text().to_string();
        alias_set.insert(access_path, preferred_name, alias_decl, ref_field, ref_var);
        alias_set.add_disable_check(value_expr.get_range());
    }
}

/// Runtime members that `resolve_member` does not bridge (std table members like `string.gsub`):
/// find the member declaration by name in the declaration file using the prefix TableConst identity.
fn runtime_member_id(
    semantic_model: &SemanticModel<'_>,
    index_expr: &LuaIndexExpr,
    name: &str,
) -> Option<SemanticId> {
    let prefix = index_expr.get_prefix_expr()?;
    let prefix_ty = semantic_model.type_of_expr(prefix.get_syntax_id());
    let LuaType::TableConst(table) = prefix_ty else {
        return None;
    };
    let facts = semantic_model.file_facts_of(table.file_id)?;
    facts
        .members
        .iter()
        .find(|member| member.value_syntax.is_some() && member.key.name() == Some(name))
        .map(|member| member.id.clone())
}

/// Root name declaration of a pure dot-chain expression.
fn find_ref_var_decl_id(semantic_model: &SemanticModel<'_>, expr: &LuaExpr) -> Option<SemanticId> {
    let mut prefix = expr.clone();
    while let LuaExpr::IndexExpr(index_expr) = prefix {
        match index_expr.get_prefix_expr() {
            Some(LuaExpr::NameExpr(name_expr)) => {
                return semantic_model.resolve_name(name_expr.get_position());
            }
            Some(LuaExpr::IndexExpr(prefix_index_expr)) => {
                prefix = LuaExpr::IndexExpr(prefix_index_expr);
            }
            _ => return None,
        }
    }
    None
}

fn is_only_dot_index_expr(expr: &LuaExpr) -> Option<bool> {
    let mut index_expr = match expr {
        LuaExpr::IndexExpr(index_expr) => index_expr.clone(),
        _ => return Some(false),
    };
    loop {
        let index_token = index_expr.get_index_token()?;
        if !index_token.is_dot() {
            return Some(false);
        }
        match index_expr.get_prefix_expr() {
            Some(LuaExpr::NameExpr(_)) => return Some(true),
            Some(LuaExpr::IndexExpr(prefix_index_expr)) => {
                index_expr = prefix_index_expr;
            }
            _ => return Some(false),
        }
    }
}

#[derive(Debug)]
struct LocalAliasSet {
    local_alias_stack: Vec<HashMap<String, LocalAliasInfo>>,
    disable_check: HashSet<TextRange>,
}

#[derive(Debug)]
struct LocalAliasInfo {
    ref_var: SemanticId,
    ref_field: Option<SemanticId>,
    preferred_name: String,
    alias_decl: SemanticId,
}

impl LocalAliasSet {
    fn new() -> Self {
        LocalAliasSet {
            local_alias_stack: vec![HashMap::new()],
            disable_check: HashSet::new(),
        }
    }

    fn push(&mut self) {
        self.local_alias_stack.push(HashMap::new());
    }

    fn pop(&mut self) {
        self.local_alias_stack.pop();
    }

    fn insert(
        &mut self,
        access_path: String,
        preferred_name: String,
        alias_decl: SemanticId,
        ref_field: Option<SemanticId>,
        ref_var: SemanticId,
    ) {
        if let Some(map) = self.local_alias_stack.last_mut() {
            map.insert(
                access_path,
                LocalAliasInfo {
                    ref_var,
                    ref_field,
                    preferred_name,
                    alias_decl,
                },
            );
        }
    }

    fn get(&self, access_path: &str) -> Option<&LocalAliasInfo> {
        for map in self.local_alias_stack.iter().rev() {
            if let Some(item) = map.get(access_path) {
                return Some(item);
            }
        }
        None
    }

    fn add_disable_check(&mut self, range: TextRange) {
        self.disable_check.insert(range);
    }

    fn is_disable_check(&self, range: &TextRange) -> bool {
        self.disable_check.contains(range)
    }
}

/// Conservative, file-wide mutation evidence, including writes in nested functions.
/// We do not infer a call order or restore aliases after assignments.
#[derive(Default)]
struct AliasWrites {
    variables: HashSet<SemanticId>,
    fields: HashSet<SemanticId>,
    paths: HashSet<(SemanticId, String)>,
    dynamic_roots: HashSet<SemanticId>,
}

impl AliasWrites {
    fn collect(&mut self, model: &SemanticModel<'_>, assign: &LuaAssignStat) {
        let (vars, _) = assign.get_var_and_expr_list();
        for var in vars {
            match var {
                LuaVarExpr::NameExpr(name) => {
                    if let Some(decl) = model.resolve_name(name.get_position()) {
                        self.variables.insert(decl);
                    }
                }
                LuaVarExpr::IndexExpr(index) => {
                    let resolved = model.resolve_member(&index);
                    if let Some(field) = resolved.as_ref().and_then(|r| r.member_id.clone()) {
                        self.fields.insert(field);
                    }
                    // Only direct fields are alias candidates. Nested writes do not
                    // replace the referenced table itself.
                    if let Some(LuaExpr::NameExpr(root)) = index.get_prefix_expr()
                        && let Some(decl) = model.resolve_name(root.get_position())
                    {
                        match index.get_index_key() {
                            Some(LuaIndexKey::Name(name)) => {
                                self.paths.insert((decl, name.get_name_text().to_string()));
                            }
                            Some(LuaIndexKey::String(value)) => {
                                self.paths.insert((decl, value.get_value().to_string()));
                            }
                            Some(LuaIndexKey::Integer(_) | LuaIndexKey::Idx(_)) => {}
                            _ => {
                                self.dynamic_roots.insert(decl);
                            }
                        }
                    }
                }
            }
        }
    }
}

fn check_index_expr_preference(
    context: &mut CheckContext<'_>,
    alias_set: &LocalAliasSet,
    semantic_model: &SemanticModel<'_>,
    index_expr: &LuaIndexExpr,
) {
    if alias_set.is_disable_check(&index_expr.get_range()) {
        return;
    }
    let expr = LuaExpr::IndexExpr(index_expr.clone());
    if !is_only_dot_index_expr(&expr).unwrap_or(false) {
        return;
    }
    // M0: only single-level member references.
    if !matches!(index_expr.get_prefix_expr(), Some(LuaExpr::NameExpr(_))) {
        return;
    }
    let Some(access_path) = index_expr.get_access_path() else {
        return;
    };
    let Some(alias_info) = alias_set.get(&access_path) else {
        return;
    };
    // A same-named local in an inner scope is not the original alias.
    if semantic_model
        .file_facts()
        .and_then(|facts| {
            facts.find_visible_decl_before_offset(
                &alias_info.preferred_name,
                index_expr.get_position(),
            )
        })
        .map(|decl| &decl.id)
        != Some(&alias_info.alias_decl)
    {
        return;
    }
    // Same root variable?
    let var_expr = first_name_expr(index_expr);
    let Some(var_expr) = var_expr else {
        return;
    };
    if !semantic_model.is_reference_to(
        rowan::NodeOrToken::Node(var_expr.syntax().clone()),
        &alias_info.ref_var,
    ) {
        return;
    }
    // Same outermost field?
    if let Some(ref_field) = &alias_info.ref_field {
        let Some(resolved) = semantic_model.resolve_member(index_expr) else {
            return;
        };
        let check_field = resolved
            .member_id
            .or_else(|| runtime_member_id(semantic_model, index_expr, &resolved.name));
        if check_field.as_ref() != Some(ref_field) {
            return;
        }
    }
    context.add_diagnostic(
        DiagnosticCode::PreferredLocalAlias,
        index_expr.get_range(),
        t!(
            "Prefer use local alias variable `%{name}`",
            name = alias_info.preferred_name
        ),
    );
}

fn first_name_expr(index_expr: &LuaIndexExpr) -> Option<LuaExpr> {
    let mut index_expr = index_expr.clone();
    loop {
        match index_expr.get_prefix_expr() {
            Some(LuaExpr::NameExpr(name_expr)) => return Some(LuaExpr::NameExpr(name_expr)),
            Some(LuaExpr::IndexExpr(prefix_index_expr)) => {
                index_expr = prefix_index_expr;
            }
            _ => return None,
        }
    }
}
