//! LuaCATS value references in alias definitions (`Namespace.Constant`).
use super::prelude::*;
use emmylua_parser::PathTrait;

#[derive(Clone)]
struct ValueLocation {
    file: FileId,
    syntax: LuaSyntaxId,
    owner: SemanticId,
}

impl SemanticModel<'_> {
    pub(super) fn resolve_alias_value_references(&self, def: &TypeDef, ty: &LuaType) -> LuaType {
        if !ty.any_type(|part| matches!(part, LuaType::StrTplRef(_))) {
            return ty.clone();
        }
        type_eval::map_type(ty, &mut |part| {
            let LuaType::StrTplRef(template) = part else {
                return None;
            };
            if !template.get_prefix().is_empty()
                || !template.get_suffix().is_empty()
                || matches!(template.get_tpl_id(), GenericTplId::Type(_))
                || def
                    .generic_params
                    .iter()
                    .any(|param| param.name == template.get_name())
            {
                return None;
            }
            let head = template.get_name().split('.').next().unwrap_or_default();
            let offset = def
                .owner_syntax
                .filter(|owner| {
                    self.file_facts_of(def.file_id).is_some_and(|facts| {
                        facts.decls.iter().any(|decl| {
                            decl.owner_syntax == Some(*owner)
                                && decl.name == head
                                && decl.kind.is_local()
                        })
                    })
                })
                .map(|owner| owner.get_range().end())
                .unwrap_or(def.name_range.start());
            Some(
                self.alias_value_path(
                    def.file_id,
                    offset,
                    template.get_name(),
                    &mut HashSet::new(),
                )
                .unwrap_or(LuaType::Unknown),
            )
        })
    }

    fn alias_value_path(
        &self,
        file: FileId,
        offset: TextSize,
        path: &str,
        visiting: &mut HashSet<(FileId, LuaSyntaxId)>,
    ) -> Option<LuaType> {
        let parts: Vec<_> = path.split('.').collect();
        // These are symbol paths, never code to execute or string templates.
        if parts.iter().any(|part| {
            let mut chars = part.chars();
            !chars.next().is_some_and(|c| c == '_' || c.is_alphabetic())
                || !chars.all(|c| c == '_' || c.is_alphanumeric())
        }) {
            return None;
        }
        let head = *parts.first()?;
        let facts = self.file_facts_of(file)?;
        let decl = if let Some(decl) = facts.find_visible_decl_before_offset(head, offset) {
            decl.clone()
        } else {
            let id = self
                .global_decls_named_for_file(file, head)
                .into_iter()
                .next()?;
            let SemanticId::Decl(key) = &id else {
                return None;
            };
            self.file_facts_of(key.file_id)?.decl_by_id(&id)?.clone()
        };
        let global = matches!(decl.kind, DeclKind::Global);
        let mut location = ValueLocation {
            file: decl.file_id,
            syntax: decl.value_expr_syntax?,
            owner: decl.id,
        };
        let mut prefix = head.to_string();
        for name in parts.iter().skip(1) {
            let mut owners = vec![location.owner.clone()];
            if let Some(LuaType::TableConst(table)) = self.alias_value_at(&location, visiting) {
                owners.push(SemanticId::member(table.file_id, table.value));
            }
            if global {
                owners.push(SemanticId::name(prefix.as_str().into()));
            }
            let mut found = None;
            for owner in owners {
                for reference in self.members_of_owner_named(&owner, name).iter() {
                    let facts = self.file_facts_of(reference.file_id)?;
                    let member = facts.member_by_id(&reference.id)?;
                    // The runtime initializer is authoritative here. In SDKs the
                    // field's annotation often refers back to this very alias.
                    if matches!(member.owner, SemanticId::TypeDef(_)) {
                        continue;
                    }
                    if let Some(syntax) = member.value_syntax {
                        found = Some(ValueLocation {
                            file: reference.file_id,
                            syntax,
                            owner: reference.id.clone(),
                        });
                        break;
                    }
                }
                if found.is_some() {
                    break;
                }
            }
            location = found?;
            prefix.push('.');
            prefix.push_str(name);
        }
        self.alias_value_at(&location, visiting)
    }

    fn alias_value_at(
        &self,
        location: &ValueLocation,
        visiting: &mut HashSet<(FileId, LuaSyntaxId)>,
    ) -> Option<LuaType> {
        let key = (location.file, location.syntax);
        if !visiting.insert(key) {
            return None;
        }
        let result = (|| {
            let tree = self.syntax_tree_of(location.file)?;
            let node = location.syntax.to_node_from_root(&tree.get_red_root())?;
            let expr = LuaExpr::cast(node)?;
            let path = match &expr {
                LuaExpr::NameExpr(name) => name.get_name_text(),
                LuaExpr::IndexExpr(index) => index.get_access_path().map(|path| path.to_string()),
                _ => None,
            };
            if let Some(path) = path {
                return self.alias_value_path(location.file, expr.get_position(), &path, visiting);
            }
            // VM expression inference preserves literal values (decl/member
            // inference can widen 1 to integer or apply a circular annotation).
            Some(self.model_for(location.file).type_of_expr(location.syntax))
        })();
        visiting.remove(&key);
        result
    }
}
