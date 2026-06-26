use super::layout::{align_up, FieldLayout, StructLayout};
use super::types::LlvmType;
use crate::lalr::{
    eval_const_exp_with, func_param_btype, BType, CompUnit, FuncParam, GlobalDef, InitVal,
    StructDef, Type, VarDecl,
};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub(crate) struct FuncSig {
    pub(crate) ret: LlvmType,
    pub(crate) is_async: bool,
    pub(crate) params: Vec<LlvmType>,
}

#[derive(Debug, Default)]
pub(crate) struct ModuleCtx {
    pub(crate) structs: HashMap<String, StructDef>,
    pub(crate) layouts: HashMap<String, StructLayout>,
    pub(crate) funcs: HashMap<String, FuncSig>,
    pub(crate) globals: HashMap<String, LlvmType>,
    pub(crate) constants: HashMap<String, i32>,
}

impl ModuleCtx {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn scan(&mut self, ast: &CompUnit) -> Result<(), String> {
        self.constants.extend(precollect_global_constants(ast));
        for glob_def in &ast.global_defs {
            if let GlobalDef::StructDef(def) = glob_def {
                self.structs.insert(def.name.clone(), def.clone());
            }
        }
        self.insert_runtime_funcs();
        for glob_def in &ast.global_defs {
            match glob_def {
                GlobalDef::FuncDef(func) => {
                    let ret = type_to_llvm(&func.func_type);
                    let params = func
                        .func_params
                        .iter()
                        .map(|param| {
                            param_resolved_btype(param, self).map(|ty| LlvmType::from_btype(&ty))
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    self.funcs.insert(
                        func.ident.clone(),
                        FuncSig {
                            ret,
                            is_async: func.is_async,
                            params,
                        },
                    );
                }
                GlobalDef::GlobalDecl(ty, decls) => {
                    for decl in decls {
                        let name = var_decl_name(&decl.var);
                        let value_ty = decl_type(ty, &decl.var, self)?;
                        if matches!(ty, Type::Const(_)) && matches!(value_ty, LlvmType::I32) {
                            if let Some(init) = &decl.init {
                                if let Some(value) = init_const(init, self) {
                                    self.constants.insert(name, value);
                                }
                            }
                        } else {
                            self.globals.insert(name, value_ty);
                        }
                    }
                }
                GlobalDef::StructDef(_) => {}
            }
        }
        Ok(())
    }

    fn insert_runtime_funcs(&mut self) {
        let i32_ty = LlvmType::I32;
        let void_ty = LlvmType::Void;
        let ptr_i32 = LlvmType::Ptr(Box::new(LlvmType::I32));
        for (name, ret, params) in [
            ("getint", i32_ty.clone(), vec![]),
            ("getch", i32_ty.clone(), vec![]),
            ("getarray", i32_ty.clone(), vec![ptr_i32.clone()]),
            ("putint", void_ty.clone(), vec![i32_ty.clone()]),
            ("putch", i32_ty.clone(), vec![i32_ty.clone()]),
            ("putarray", void_ty.clone(), vec![i32_ty.clone(), ptr_i32]),
            ("starttime", void_ty.clone(), vec![]),
            ("stoptime", void_ty.clone(), vec![]),
        ] {
            self.funcs.insert(
                name.to_string(),
                FuncSig {
                    ret,
                    is_async: false,
                    params,
                },
            );
        }
    }

    pub(crate) fn resolve_struct_layouts(&mut self) -> Result<(), String> {
        let names = self.structs.keys().cloned().collect::<Vec<_>>();
        for name in names {
            self.ensure_struct_layout(&name, &mut Vec::new())?;
        }
        Ok(())
    }

    fn ensure_struct_layout(
        &mut self,
        name: &str,
        visiting: &mut Vec<String>,
    ) -> Result<StructLayout, String> {
        if let Some(layout) = self.layouts.get(name) {
            return Ok(layout.clone());
        }
        if visiting.iter().any(|item| item == name) {
            return Err(format!("Cyclic struct definition detected: {}", name));
        }
        visiting.push(name.to_string());
        let def = self
            .structs
            .get(name)
            .ok_or_else(|| format!("Unknown struct type {}", name))?
            .clone();
        let mut fields = Vec::new();
        let mut offset = 0usize;
        let mut align = 1usize;
        for field in &def.fields {
            for decl in &field.decls {
                let ty = decl_type(&field.ty, &decl.var, self)?;
                self.ensure_type_layouts(&ty, visiting)?;
                let field_align = ty.try_align(&self.layouts)?;
                let field_size = ty.try_size(&self.layouts)?;
                align = align.max(field_align);
                offset = align_up(offset, field_align);
                let index = fields.len();
                fields.push(FieldLayout {
                    name: var_decl_name(&decl.var),
                    ty,
                    index,
                });
                offset += field_size;
            }
        }
        let size = align_up(offset, align);
        visiting.pop();
        let layout = StructLayout {
            fields,
            size,
            align,
        };
        self.layouts.insert(name.to_string(), layout.clone());
        Ok(layout)
    }

    fn ensure_type_layouts(
        &mut self,
        ty: &LlvmType,
        visiting: &mut Vec<String>,
    ) -> Result<(), String> {
        match ty {
            LlvmType::Struct(inner) => {
                self.ensure_struct_layout(inner, visiting)?;
                Ok(())
            }
            LlvmType::Array(_, inner) => self.ensure_type_layouts(inner, visiting),
            LlvmType::I32 | LlvmType::Void | LlvmType::Ptr(_) | LlvmType::Promise(_) => Ok(()),
        }
    }

    pub(crate) fn field(
        &self,
        struct_name: &str,
        field_name: &str,
    ) -> Result<&FieldLayout, String> {
        self.layouts
            .get(struct_name)
            .ok_or_else(|| format!("Unknown struct type {}", struct_name))?
            .fields
            .iter()
            .find(|field| field.name == field_name)
            .ok_or_else(|| format!("Struct {} has no field {}", struct_name, field_name))
    }
}

fn precollect_global_constants(ast: &CompUnit) -> HashMap<String, i32> {
    let mut constants = HashMap::new();
    let mut pending = Vec::new();
    for glob_def in &ast.global_defs {
        let GlobalDef::GlobalDecl(ty, decls) = glob_def else {
            continue;
        };
        if !matches!(ty, Type::Const(BType::I32)) {
            continue;
        }
        for decl in decls {
            let (VarDecl::Ident(name), Some(init)) = (&decl.var, &decl.init) else {
                continue;
            };
            let Some(exp) = init_exp(init) else {
                continue;
            };
            pending.push((name.clone(), exp.clone()));
        }
    }

    let mut changed = true;
    while changed {
        changed = false;
        for (name, exp) in &pending {
            if constants.contains_key(name) {
                continue;
            }
            if let Some(value) = eval_const_exp_with(exp, &|name| constants.get(name).copied()) {
                constants.insert(name.clone(), value);
                changed = true;
            }
        }
    }
    constants
}

pub(crate) fn type_to_llvm(ty: &Type) -> LlvmType {
    match ty {
        Type::BType(btype) | Type::Const(btype) => LlvmType::from_btype(btype),
    }
}

pub(crate) fn decl_type(ty: &Type, var: &VarDecl, module: &ModuleCtx) -> Result<LlvmType, String> {
    decl_type_with_constants(ty, var, module, &HashMap::new())
}

pub(crate) fn decl_type_with_constants(
    ty: &Type,
    var: &VarDecl,
    module: &ModuleCtx,
    constants: &HashMap<String, i32>,
) -> Result<LlvmType, String> {
    let base = type_to_llvm(ty);
    apply_var_dims(base, var, module, constants)
}

fn apply_var_dims(
    base: LlvmType,
    var: &VarDecl,
    module: &ModuleCtx,
    constants: &HashMap<String, i32>,
) -> Result<LlvmType, String> {
    let mut dims = Vec::new();
    collect_var_dims(var, module, constants, &mut dims)?;
    let mut ty = base;
    for dim in dims.into_iter().rev() {
        ty = LlvmType::Array(dim, Box::new(ty));
    }
    Ok(ty)
}

fn collect_var_dims(
    var: &VarDecl,
    module: &ModuleCtx,
    constants: &HashMap<String, i32>,
    dims: &mut Vec<usize>,
) -> Result<(), String> {
    match var {
        VarDecl::Ident(_) => Ok(()),
        VarDecl::Array(inner, len) => {
            collect_var_dims(inner, module, constants, dims)?;
            let len = eval_const_exp_with(len, &|name| {
                constants
                    .get(name)
                    .copied()
                    .or_else(|| module.constants.get(name).copied())
            })
            .ok_or_else(|| "Array length must be a constant expression".to_string())?;
            if len < 0 {
                return Err("Array length must be non-negative".to_string());
            }
            dims.push(len as usize);
            Ok(())
        }
    }
}

pub(crate) fn param_resolved_btype(param: &FuncParam, module: &ModuleCtx) -> Result<BType, String> {
    if let Some(dims) = &param.array_dims {
        let dims = dims
            .iter()
            .map(|dim| {
                let value = eval_const_exp_with(dim, &|name| module.constants.get(name).copied())
                    .ok_or_else(|| {
                    "Array parameter dimensions must be constant expressions".to_string()
                })?;
                if value < 0 {
                    return Err("Array parameter dimensions must be non-negative".to_string());
                }
                Ok(value as usize)
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(func_param_btype(param.btype.clone(), dims))
    } else {
        Ok(param.btype.clone())
    }
}

pub(crate) fn var_decl_name(var: &VarDecl) -> String {
    match var {
        VarDecl::Ident(name) => name.clone(),
        VarDecl::Array(inner, _) => var_decl_name(inner),
    }
}

pub(crate) fn init_const(init: &InitVal, module: &ModuleCtx) -> Option<i32> {
    match init {
        InitVal::Spanned(init, _) => init_const(init, module),
        InitVal::Exp(exp) => eval_const_exp_with(exp, &|name| module.constants.get(name).copied()),
        InitVal::Arr(_) => None,
    }
}

fn init_exp(init: &InitVal) -> Option<&crate::lalr::Exp> {
    match init {
        InitVal::Spanned(init, _) => init_exp(init),
        InitVal::Exp(exp) => Some(exp),
        InitVal::Arr(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lalr::{Exp, SingleDecl, StructField};

    fn ident_decl(name: &str) -> SingleDecl {
        SingleDecl {
            span: None,
            var: VarDecl::Ident(name.to_string()),
            init: None,
        }
    }

    #[test]
    fn resolve_struct_layouts_reports_unknown_struct() {
        let mut module = ModuleCtx::new();
        module.structs.insert(
            "holder".to_string(),
            StructDef {
                span: None,
                name: "holder".to_string(),
                fields: vec![StructField {
                    span: None,
                    ty: Type::BType(BType::Struct("missing".to_string())),
                    decls: vec![ident_decl("value")],
                }],
            },
        );

        let err = module.resolve_struct_layouts().unwrap_err();
        assert!(err.contains("Unknown struct type missing"));
    }

    #[test]
    fn resolve_struct_layouts_reports_nested_array_struct_cycle() {
        let mut module = ModuleCtx::new();
        module.structs.insert(
            "node".to_string(),
            StructDef {
                span: None,
                name: "node".to_string(),
                fields: vec![StructField {
                    span: None,
                    ty: Type::BType(BType::Struct("node".to_string())),
                    decls: vec![SingleDecl {
                        span: None,
                        var: VarDecl::Array(
                            Box::new(VarDecl::Ident("children".to_string())),
                            Exp::Number(2),
                        ),
                        init: None,
                    }],
                }],
            },
        );

        let err = module.resolve_struct_layouts().unwrap_err();
        assert!(err.contains("Cyclic struct definition detected: node"));
    }
}
