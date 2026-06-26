use super::async_cfg::{
    build_async_cfg, AsyncCfgFunction, AsyncOp, AsyncTerminator, AwaitTerminator,
};
use super::builder::{FunctionCtx, Value, VarInfo};
use super::layout::{align_up, StructLayout, TARGET_LAYOUT};
use super::module::{
    decl_type_with_constants, param_resolved_btype, type_to_llvm, var_decl_name, ModuleCtx,
};
use super::names::sanitize_ident;
use super::sync::{emit_cond_br, emit_exp, emit_lvalue, init_store, store_value_to_ptr};
use super::types::LlvmType;
use crate::lalr::{
    eval_const_exp_with, BType, Exp, FuncDef, InitVal, SingleDecl, Stmt, Type, UnaryOp, VarDecl,
};
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

#[derive(Debug, Clone)]
struct FrameField {
    frame_type: String,
    index: usize,
    ty: LlvmType,
}

#[derive(Debug, Clone)]
struct AsyncFrameLayout {
    type_name: String,
    fields: HashMap<String, FrameField>,
    ordered_fields: Vec<(String, LlvmType)>,
    size: usize,
}

#[derive(Debug, Clone)]
struct AwaitPointInfo {
    state: i32,
    child_field: String,
    result_target: Option<String>,
    callback_name: String,
}

struct LocalRenamer {
    scopes: Vec<HashMap<String, String>>,
    next_id: usize,
}

impl LocalRenamer {
    fn new() -> Self {
        Self {
            scopes: Vec::new(),
            next_id: 0,
        }
    }

    fn rename_func(mut self, func: &FuncDef) -> FuncDef {
        self.push_scope();
        for param in &func.func_params {
            self.define(param.name.clone(), param.name.clone());
        }
        let block = self.rename_stmts(&func.block);
        self.pop_scope();
        FuncDef {
            span: func.span,
            is_async: func.is_async,
            func_type: func.func_type.clone(),
            ident: func.ident.clone(),
            func_params: func.func_params.clone(),
            block,
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn define(&mut self, original: String, renamed: String) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(original, renamed);
        }
    }

    fn lookup(&self, name: &str) -> Option<String> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    fn unique_local(&mut self, name: &str) -> String {
        let id = self.next_id;
        self.next_id += 1;
        format!("__sysy_local_{}_{}", id, name)
    }

    fn rename_stmts(&mut self, stmts: &[Stmt]) -> Vec<Stmt> {
        stmts.iter().map(|stmt| self.rename_stmt(stmt)).collect()
    }

    fn rename_stmt(&mut self, stmt: &Stmt) -> Stmt {
        match stmt {
            Stmt::Spanned(stmt, span) => Stmt::Spanned(Box::new(self.rename_stmt(stmt)), *span),
            Stmt::Block(stmts) => {
                self.push_scope();
                let renamed = self.rename_stmts(stmts);
                self.pop_scope();
                Stmt::Block(renamed)
            }
            Stmt::Assign(lhs, rhs) => Stmt::Assign(self.rename_exp(lhs), self.rename_exp(rhs)),
            Stmt::Decl(ty, decls) => {
                let mut renamed_decls = Vec::new();
                for decl in decls {
                    let init = decl.init.as_ref().map(|init| self.rename_init(init));
                    let (var, original, renamed) = self.rename_decl_var(&decl.var);
                    self.define(original, renamed);
                    renamed_decls.push(SingleDecl {
                        span: decl.span,
                        var,
                        init,
                    });
                }
                Stmt::Decl(ty.clone(), renamed_decls)
            }
            Stmt::Exp(exp) => Stmt::Exp(self.rename_exp(exp)),
            Stmt::If(cond, then_stmt) => {
                Stmt::If(self.rename_exp(cond), Box::new(self.rename_stmt(then_stmt)))
            }
            Stmt::IfElse(cond, then_stmt, else_stmt) => Stmt::IfElse(
                self.rename_exp(cond),
                Box::new(self.rename_stmt(then_stmt)),
                Box::new(self.rename_stmt(else_stmt)),
            ),
            Stmt::While(cond, body) => {
                Stmt::While(self.rename_exp(cond), Box::new(self.rename_stmt(body)))
            }
            Stmt::PromiseWait(exp) => Stmt::PromiseWait(self.rename_exp(exp)),
            Stmt::Return(Some(exp)) => Stmt::Return(Some(self.rename_exp(exp))),
            Stmt::Return(None) => Stmt::Return(None),
            Stmt::Continue => Stmt::Continue,
            Stmt::Break => Stmt::Break,
            Stmt::Empty => Stmt::Empty,
        }
    }

    fn rename_decl_var(&mut self, var: &VarDecl) -> (VarDecl, String, String) {
        match var {
            VarDecl::Ident(name) => {
                let renamed = self.unique_local(name);
                (VarDecl::Ident(renamed.clone()), name.clone(), renamed)
            }
            VarDecl::Array(inner, dim) => {
                let renamed_dim = self.rename_exp(dim);
                let (renamed_inner, original, renamed) = self.rename_decl_var(inner);
                (
                    VarDecl::Array(Box::new(renamed_inner), renamed_dim),
                    original,
                    renamed,
                )
            }
        }
    }

    fn rename_init(&mut self, init: &InitVal) -> InitVal {
        match init {
            InitVal::Spanned(init, span) => {
                InitVal::Spanned(Box::new(self.rename_init(init)), *span)
            }
            InitVal::Exp(exp) => InitVal::Exp(self.rename_exp(exp)),
            InitVal::Arr(items) => {
                InitVal::Arr(items.iter().map(|item| self.rename_init(item)).collect())
            }
        }
    }

    fn rename_exp(&mut self, exp: &Exp) -> Exp {
        match exp {
            Exp::Spanned(exp, span) => Exp::Spanned(Box::new(self.rename_exp(exp)), *span),
            Exp::Ident(name) => Exp::Ident(self.lookup(name).unwrap_or_else(|| name.clone())),
            Exp::Number(value) => Exp::Number(*value),
            Exp::UnaryExp(op, inner) => Exp::UnaryExp(op.clone(), Box::new(self.rename_exp(inner))),
            Exp::BinaryExp(op, lhs, rhs) => Exp::BinaryExp(
                op.clone(),
                Box::new(self.rename_exp(lhs)),
                Box::new(self.rename_exp(rhs)),
            ),
            Exp::FuncCall(name, args) => Exp::FuncCall(
                name.clone(),
                args.iter().map(|arg| self.rename_exp(arg)).collect(),
            ),
            Exp::ArrGet(base, index) => Exp::ArrGet(
                Box::new(self.rename_exp(base)),
                Box::new(self.rename_exp(index)),
            ),
            Exp::Field(base, field) => Exp::Field(Box::new(self.rename_exp(base)), field.clone()),
            Exp::PtrField(base, field) => {
                Exp::PtrField(Box::new(self.rename_exp(base)), field.clone())
            }
            Exp::New(ty) => Exp::New(ty.clone()),
            Exp::Await(inner) => Exp::Await(Box::new(self.rename_exp(inner))),
            Exp::Sleep(inner) => Exp::Sleep(Box::new(self.rename_exp(inner))),
            Exp::PromiseWait(inner) => Exp::PromiseWait(Box::new(self.rename_exp(inner))),
        }
    }
}

fn rename_async_locals(func: &FuncDef) -> FuncDef {
    LocalRenamer::new().rename_func(func)
}

fn async_frame_layout(
    module: &ModuleCtx,
    frame_type: &str,
    params: &[(String, LlvmType)],
    stmts: &[Stmt],
    frame_locals: &HashSet<String>,
    extra_fields: &HashMap<String, LlvmType>,
    await_count: usize,
) -> Result<AsyncFrameLayout, String> {
    let mut fields = HashMap::new();
    let mut ordered_fields = Vec::new();
    let mut offset = 0usize;
    let mut max_align = 1usize;
    add_frame_field(
        &mut fields,
        &mut ordered_fields,
        &mut offset,
        &mut max_align,
        &module.layouts,
        frame_type,
        "__promise".to_string(),
        LlvmType::Promise(Box::new(LlvmType::Void)),
    )?;
    for (name, ty) in params {
        add_frame_field(
            &mut fields,
            &mut ordered_fields,
            &mut offset,
            &mut max_align,
            &module.layouts,
            frame_type,
            name.clone(),
            ty.clone(),
        )?;
    }
    let mut constants = module.constants.clone();
    collect_async_decl_fields(
        module,
        frame_type,
        stmts,
        frame_locals,
        &mut fields,
        &mut ordered_fields,
        &mut offset,
        &mut max_align,
        &mut constants,
    )?;
    for (name, ty) in extra_fields {
        add_frame_field(
            &mut fields,
            &mut ordered_fields,
            &mut offset,
            &mut max_align,
            &module.layouts,
            frame_type,
            name.clone(),
            ty.clone(),
        )?;
    }
    for idx in 0..await_count {
        add_frame_field(
            &mut fields,
            &mut ordered_fields,
            &mut offset,
            &mut max_align,
            &module.layouts,
            frame_type,
            format!("__await{}", idx),
            LlvmType::Promise(Box::new(LlvmType::I32)),
        )?;
    }
    add_frame_field(
        &mut fields,
        &mut ordered_fields,
        &mut offset,
        &mut max_align,
        &module.layouts,
        frame_type,
        "__return".to_string(),
        LlvmType::I32,
    )?;
    Ok(AsyncFrameLayout {
        type_name: frame_type.to_string(),
        fields,
        ordered_fields,
        size: align_up(offset, max_align),
    })
}

fn add_frame_field(
    fields: &mut HashMap<String, FrameField>,
    ordered_fields: &mut Vec<(String, LlvmType)>,
    offset: &mut usize,
    max_align: &mut usize,
    structs: &HashMap<String, StructLayout>,
    frame_type: &str,
    name: String,
    ty: LlvmType,
) -> Result<(), String> {
    if fields.contains_key(&name) {
        return Ok(());
    }
    let field_align = ty.try_align(structs)?;
    *max_align = (*max_align).max(field_align);
    *offset = align_up(*offset, field_align);
    let index = ordered_fields.len();
    fields.insert(
        name.clone(),
        FrameField {
            frame_type: frame_type.to_string(),
            index,
            ty: ty.clone(),
        },
    );
    ordered_fields.push((name, ty.clone()));
    *offset += ty.try_size(structs)?;
    Ok(())
}

fn collect_async_decl_fields(
    module: &ModuleCtx,
    frame_type: &str,
    stmts: &[Stmt],
    frame_locals: &HashSet<String>,
    fields: &mut HashMap<String, FrameField>,
    ordered_fields: &mut Vec<(String, LlvmType)>,
    offset: &mut usize,
    max_align: &mut usize,
    constants: &mut HashMap<String, i32>,
) -> Result<(), String> {
    for stmt in stmts {
        match stmt {
            Stmt::Spanned(stmt, _) => collect_async_decl_fields(
                module,
                frame_type,
                std::slice::from_ref(stmt.as_ref()),
                frame_locals,
                fields,
                ordered_fields,
                offset,
                max_align,
                constants,
            )?,
            Stmt::Decl(ty, decls) => {
                for decl in decls {
                    let name = var_decl_name(&decl.var);
                    let value_ty = decl_type_with_constants(ty, &decl.var, module, constants)?;
                    if !frame_locals.contains(&name) {
                        record_const_decl(constants, ty, decl);
                        continue;
                    }
                    add_frame_field(
                        fields,
                        ordered_fields,
                        offset,
                        max_align,
                        &module.layouts,
                        frame_type,
                        name,
                        value_ty,
                    )?;
                    record_const_decl(constants, ty, decl);
                }
            }
            Stmt::Block(stmts) => {
                let mut scoped_constants = constants.clone();
                collect_async_decl_fields(
                    module,
                    frame_type,
                    stmts,
                    frame_locals,
                    fields,
                    ordered_fields,
                    offset,
                    max_align,
                    &mut scoped_constants,
                )?
            }
            Stmt::If(_, then_stmt) => {
                let mut scoped_constants = constants.clone();
                collect_async_decl_fields(
                    module,
                    frame_type,
                    std::slice::from_ref(then_stmt),
                    frame_locals,
                    fields,
                    ordered_fields,
                    offset,
                    max_align,
                    &mut scoped_constants,
                )?
            }
            Stmt::IfElse(_, then_stmt, else_stmt) => {
                let mut then_constants = constants.clone();
                collect_async_decl_fields(
                    module,
                    frame_type,
                    std::slice::from_ref(then_stmt),
                    frame_locals,
                    fields,
                    ordered_fields,
                    offset,
                    max_align,
                    &mut then_constants,
                )?;
                let mut else_constants = constants.clone();
                collect_async_decl_fields(
                    module,
                    frame_type,
                    std::slice::from_ref(else_stmt),
                    frame_locals,
                    fields,
                    ordered_fields,
                    offset,
                    max_align,
                    &mut else_constants,
                )?;
            }
            Stmt::While(_, body) => {
                let mut scoped_constants = constants.clone();
                collect_async_decl_fields(
                    module,
                    frame_type,
                    std::slice::from_ref(body),
                    frame_locals,
                    fields,
                    ordered_fields,
                    offset,
                    max_align,
                    &mut scoped_constants,
                )?
            }
            _ => {}
        }
    }
    Ok(())
}

fn collect_await_points(
    module: &ModuleCtx,
    safe_func_name: &str,
    cfg: &AsyncCfgFunction,
) -> Vec<AwaitPointInfo> {
    let _ = module;
    cfg.await_points()
        .into_iter()
        .map(|await_term| AwaitPointInfo {
            state: await_term.state,
            child_field: format!("__await{}", (await_term.state - 1) as usize),
            result_target: await_term.result_target.clone(),
            callback_name: format!("__sysy_async_cont_{}_{}", safe_func_name, await_term.state),
        })
        .collect()
}

fn record_const_decl(constants: &mut HashMap<String, i32>, ty: &Type, decl: &SingleDecl) {
    if !matches!(ty, Type::Const(BType::I32)) {
        return;
    }
    let (VarDecl::Ident(name), Some(init)) = (&decl.var, &decl.init) else {
        return;
    };
    let Some(exp) = init_exp(init) else {
        return;
    };
    if let Some(value) = eval_const_exp_with(exp, &|name| constants.get(name).copied()) {
        constants.insert(name.clone(), value);
    }
}

fn init_exp(init: &InitVal) -> Option<&Exp> {
    match init {
        InitVal::Spanned(init, _) => init_exp(init),
        InitVal::Exp(exp) => Some(exp),
        InitVal::Arr(_) => None,
    }
}

fn collect_function_type_env(
    module: &ModuleCtx,
    func: &FuncDef,
) -> Result<HashMap<String, LlvmType>, String> {
    let mut env = HashMap::new();
    for param in &func.func_params {
        env.insert(
            param.name.clone(),
            LlvmType::from_btype(&param_resolved_btype(param, module)?),
        );
    }
    let mut constants = module.constants.clone();
    collect_decl_type_env(module, &func.block, &mut env, &mut constants)?;
    Ok(env)
}

fn collect_decl_type_env(
    module: &ModuleCtx,
    stmts: &[Stmt],
    env: &mut HashMap<String, LlvmType>,
    constants: &mut HashMap<String, i32>,
) -> Result<(), String> {
    for stmt in stmts {
        match stmt {
            Stmt::Spanned(stmt, _) => {
                collect_decl_type_env(module, std::slice::from_ref(stmt.as_ref()), env, constants)?
            }
            Stmt::Block(stmts) => {
                let mut scoped_constants = constants.clone();
                collect_decl_type_env(module, stmts, env, &mut scoped_constants)?;
            }
            Stmt::Decl(ty, decls) => {
                for decl in decls {
                    env.insert(
                        var_decl_name(&decl.var),
                        decl_type_with_constants(ty, &decl.var, module, constants)?,
                    );
                    record_const_decl(constants, ty, decl);
                }
            }
            Stmt::If(_, then_stmt) => {
                let mut scoped_constants = constants.clone();
                collect_decl_type_env(
                    module,
                    std::slice::from_ref(then_stmt),
                    env,
                    &mut scoped_constants,
                )?
            }
            Stmt::IfElse(_, then_stmt, else_stmt) => {
                let mut then_constants = constants.clone();
                collect_decl_type_env(
                    module,
                    std::slice::from_ref(then_stmt),
                    env,
                    &mut then_constants,
                )?;
                let mut else_constants = constants.clone();
                collect_decl_type_env(
                    module,
                    std::slice::from_ref(else_stmt),
                    env,
                    &mut else_constants,
                )?;
            }
            Stmt::While(_, body) => {
                let mut scoped_constants = constants.clone();
                collect_decl_type_env(
                    module,
                    std::slice::from_ref(body),
                    env,
                    &mut scoped_constants,
                )?
            }
            Stmt::Assign(_, _)
            | Stmt::Exp(_)
            | Stmt::PromiseWait(_)
            | Stmt::Continue
            | Stmt::Break
            | Stmt::Return(_)
            | Stmt::Empty => {}
        }
    }
    Ok(())
}

fn infer_await_result_type(
    module: &ModuleCtx,
    env: &HashMap<String, LlvmType>,
    cfg: &AsyncCfgFunction,
    state: i32,
) -> Option<LlvmType> {
    let await_term = cfg
        .await_points()
        .into_iter()
        .find(|await_term| await_term.state == state)?;
    match infer_exp_type(module, env, &await_term.child)? {
        LlvmType::Promise(inner) => Some(*inner),
        other => Some(other.promise_value()),
    }
}

fn infer_exp_type(
    module: &ModuleCtx,
    env: &HashMap<String, LlvmType>,
    exp: &Exp,
) -> Option<LlvmType> {
    match exp {
        Exp::Spanned(exp, _) => infer_exp_type(module, env, exp),
        Exp::Number(_) => Some(LlvmType::I32),
        Exp::Ident(name) => env
            .get(name)
            .cloned()
            .or_else(|| module.globals.get(name).cloned())
            .or_else(|| module.constants.get(name).map(|_| LlvmType::I32)),
        Exp::UnaryExp(UnaryOp::Addr, inner) => {
            infer_lvalue_type(module, env, inner).map(|ty| LlvmType::Ptr(Box::new(ty)))
        }
        Exp::UnaryExp(UnaryOp::Deref, inner) => match infer_exp_type(module, env, inner)? {
            LlvmType::Ptr(inner) => Some(*inner),
            _ => None,
        },
        Exp::UnaryExp(_, _) | Exp::BinaryExp(_, _, _) => Some(LlvmType::I32),
        Exp::New(ty) => Some(LlvmType::Ptr(Box::new(LlvmType::from_btype(ty)))),
        Exp::Await(inner) | Exp::PromiseWait(inner) => {
            Some(infer_exp_type(module, env, inner)?.promise_value())
        }
        Exp::Sleep(_) => Some(LlvmType::Promise(Box::new(LlvmType::Void))),
        Exp::FuncCall(name, _) => {
            let sig = module.funcs.get(name)?;
            if sig.is_async {
                Some(LlvmType::Promise(Box::new(sig.ret.clone())))
            } else {
                Some(sig.ret.clone())
            }
        }
        Exp::ArrGet(_, _) | Exp::Field(_, _) | Exp::PtrField(_, _) => {
            infer_lvalue_type(module, env, exp)
        }
    }
}

fn infer_lvalue_type(
    module: &ModuleCtx,
    env: &HashMap<String, LlvmType>,
    exp: &Exp,
) -> Option<LlvmType> {
    match exp {
        Exp::Spanned(exp, _) => infer_lvalue_type(module, env, exp),
        Exp::Ident(name) => env
            .get(name)
            .cloned()
            .or_else(|| module.globals.get(name).cloned()),
        Exp::UnaryExp(UnaryOp::Deref, inner) => match infer_exp_type(module, env, inner)? {
            LlvmType::Ptr(inner) => Some(*inner),
            _ => None,
        },
        Exp::ArrGet(base, _) => match infer_exp_type(module, env, base)? {
            LlvmType::Array(_, inner) => Some(*inner),
            LlvmType::Ptr(inner) => match *inner {
                LlvmType::Array(_, element) => Some(*element),
                other => Some(other),
            },
            _ => None,
        },
        Exp::Field(base, field_name) => {
            let base_ty = infer_exp_type(module, env, base)?;
            let struct_name = match base_ty {
                LlvmType::Struct(name) => name,
                LlvmType::Ptr(inner) => match *inner {
                    LlvmType::Struct(name) => name,
                    _ => return None,
                },
                _ => return None,
            };
            Some(module.field(&struct_name, field_name).ok()?.ty.clone())
        }
        Exp::PtrField(base, field_name) => {
            let base_ty = infer_exp_type(module, env, base)?;
            let struct_name = match base_ty {
                LlvmType::Ptr(inner) => match *inner {
                    LlvmType::Struct(name) => name,
                    _ => return None,
                },
                _ => return None,
            };
            Some(module.field(&struct_name, field_name).ok()?.ty.clone())
        }
        _ => None,
    }
}

fn async_field_ptr(ctx: &mut FunctionCtx<'_>, frame: &str, field: &FrameField) -> String {
    let ptr = ctx.tmp();
    ctx.emit(format!(
        "  {} = getelementptr inbounds {}, ptr {}, i32 0, i32 {}",
        ptr, field.frame_type, frame, field.index
    ));
    ptr
}

fn async_load_field(ctx: &mut FunctionCtx<'_>, frame: &str, field: &FrameField) -> Value {
    let ptr = async_field_ptr(ctx, frame, field);
    ctx.load(&ptr, &field.ty)
}

fn async_store_field(ctx: &mut FunctionCtx<'_>, frame: &str, field: &FrameField, value: &Value) {
    let ptr = async_field_ptr(ctx, frame, field);
    ctx.emit(format!(
        "  store {} {}, ptr {}",
        value.ty.llvm(),
        value.name,
        ptr
    ));
}

pub(crate) fn emit_async_function(module: &ModuleCtx, func: &FuncDef) -> Result<String, String> {
    let ret_ty = type_to_llvm(&func.func_type);
    let lowered_func = rename_async_locals(func);
    let params = func
        .func_params
        .iter()
        .map(|param| {
            param_resolved_btype(param, module)
                .map(|ty| (param.name.clone(), LlvmType::from_btype(&ty)))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let safe_name = sanitize_ident(&func.ident);
    let frame_type = format!("%async.frame.{}", safe_name);
    let cfg = build_async_cfg(&lowered_func);
    let mut frame_locals = cfg
        .live_across_awaits()
        .into_iter()
        .flat_map(|live| live.vars)
        .collect::<HashSet<_>>();
    let start_name = format!("__sysy_async_start_{}", safe_name);
    let awaits = collect_await_points(module, &safe_name, &cfg);
    let mut extra_fields = HashMap::new();
    for temp in &cfg.i32_temps {
        extra_fields.insert(temp.clone(), LlvmType::I32);
    }
    let mut type_env = collect_function_type_env(module, &lowered_func)?;
    for await_info in &awaits {
        if let Some(target) = &await_info.result_target {
            if target != "__return" {
                if target.starts_with("__sysy_cfg_await_tmp_") {
                    if let Some(await_ty) =
                        infer_await_result_type(module, &type_env, &cfg, await_info.state)
                    {
                        type_env.insert(target.clone(), await_ty.clone());
                        extra_fields.insert(target.clone(), await_ty);
                    } else {
                        extra_fields.insert(target.clone(), LlvmType::I32);
                    }
                } else {
                    frame_locals.insert(target.clone());
                }
            }
        }
    }
    let frame_layout = async_frame_layout(
        module,
        &frame_type,
        &params,
        &lowered_func.block,
        &frame_locals,
        &extra_fields,
        awaits.len(),
    )?;
    let fields = &frame_layout.fields;

    let mut out = String::new();
    let frame_fields = frame_layout
        .ordered_fields
        .iter()
        .map(|(_, ty)| ty.llvm())
        .collect::<Vec<_>>()
        .join(", ");
    let _ = writeln!(
        out,
        "{} = type {{ {} }}\n",
        frame_layout.type_name, frame_fields
    );
    let param_sig = params
        .iter()
        .enumerate()
        .map(|(idx, (_, ty))| format!("{} %arg{}", ty.llvm(), idx))
        .collect::<Vec<_>>()
        .join(", ");
    let _ = writeln!(out, "define ptr @{}({}) {{", safe_name, param_sig);
    let mut entry = FunctionCtx::new(module, LlvmType::Promise(Box::new(ret_ty.clone())), false);
    entry.emit_label("entry");
    let promise = entry.promise_new(&ret_ty)?;
    let frame = entry.tmp();
    entry.emit(format!(
        "  {} = call ptr @malloc({} {})",
        frame, TARGET_LAYOUT.malloc_size_type, frame_layout.size,
    ));
    async_store_field(
        &mut entry,
        &frame,
        frame_field(fields, "__promise")?,
        &promise,
    );
    for (idx, (name, ty)) in params.iter().enumerate() {
        let value = Value {
            name: format!("%arg{}", idx),
            ty: ty.clone(),
        };
        async_store_field(&mut entry, &frame, frame_field(fields, name)?, &value);
    }
    entry.emit(format!(
        "  call void @__sysy_pending_add(ptr {}, ptr @{}, ptr {})",
        promise.name, start_name, frame
    ));
    entry.terminate(format!("  ret ptr {}", promise.name));
    for line in entry.lines {
        let _ = writeln!(out, "{}", line);
    }
    out.push_str("}\n\n");

    out.push_str(&emit_async_cfg_function(
        module,
        &start_name,
        &ret_ty,
        fields,
        &cfg,
        &awaits,
        cfg.entry,
        None,
    )?);

    for await_info in &awaits {
        let resume_block = await_resume_block(&cfg, await_info.state)?;
        out.push_str(&emit_async_cfg_function(
            module,
            &await_info.callback_name,
            &ret_ty,
            fields,
            &cfg,
            &awaits,
            resume_block,
            Some(await_info),
        )?);
    }

    Ok(out)
}

fn emit_async_cfg_function(
    module: &ModuleCtx,
    name: &str,
    ret_ty: &LlvmType,
    fields: &HashMap<String, FrameField>,
    cfg: &AsyncCfgFunction,
    awaits: &[AwaitPointInfo],
    entry_block: usize,
    continuation: Option<&AwaitPointInfo>,
) -> Result<String, String> {
    let mut out = String::new();
    let _ = writeln!(out, "define void @{}(ptr %frame) {{", name);
    let mut ctx = FunctionCtx::new(module, LlvmType::Void, true);
    ctx.emit_label("entry");
    init_async_frame_vars(&mut ctx, fields)?;

    if let Some(await_info) = continuation {
        emit_continuation_entry(&mut ctx, fields, await_info, cfg)?;
    } else {
        let promise = current_promise(&ctx)?;
        ctx.emit(format!(
            "  call void @__sysy_promise_clear_driver(ptr {})",
            promise
        ));
        ctx.terminate(format!("  br label %{}", cfg_block_label(entry_block)));
    }

    for block in &cfg.blocks {
        ctx.emit_label(&cfg_block_label(block.id));
        for op in &block.ops {
            emit_async_cfg_op(&mut ctx, fields, op)?;
        }
        emit_async_cfg_terminator(&mut ctx, fields, cfg, awaits, ret_ty, &block.terminator)?;
    }

    ctx.emit_label("async.suspend");
    ctx.terminate("  ret void".to_string());
    for line in ctx.lines {
        let _ = writeln!(out, "{}", line);
    }
    out.push_str("}\n\n");
    Ok(out)
}

fn init_async_frame_vars(
    ctx: &mut FunctionCtx<'_>,
    fields: &HashMap<String, FrameField>,
) -> Result<(), String> {
    let promise = async_load_field(ctx, "%frame", frame_field(fields, "__promise")?);
    ctx.promise_ptr = Some(promise.name.clone());
    for (name, field) in fields.iter() {
        if is_internal_async_frame_field(name) {
            continue;
        }
        let ptr = async_field_ptr(ctx, "%frame", field);
        ctx.insert_var(
            name.clone(),
            VarInfo {
                ptr,
                ty: field.ty.clone(),
            },
        )?;
    }
    Ok(())
}

fn is_internal_async_frame_field(name: &str) -> bool {
    name == "__promise" || name == "__return" || name.starts_with("__await")
}

fn emit_continuation_entry(
    ctx: &mut FunctionCtx<'_>,
    fields: &HashMap<String, FrameField>,
    await_info: &AwaitPointInfo,
    cfg: &AsyncCfgFunction,
) -> Result<(), String> {
    let child = async_load_field(ctx, "%frame", frame_field(fields, &await_info.child_field)?);
    let ready = ctx.tmp();
    ctx.emit(format!(
        "  {} = call i32 @__sysy_promise_is_ready(ptr {})",
        ready, child.name
    ));
    let ready_bool = ctx.tmp();
    ctx.emit(format!("  {} = icmp ne i32 {}, 0", ready_bool, ready));
    let resume_label = ctx.label("cont.resume");
    let exit_label = ctx.label("cont.exit");
    ctx.terminate(format!(
        "  br i1 {}, label %{}, label %{}",
        ready_bool, resume_label, exit_label
    ));
    ctx.emit_label(&resume_label);
    if let Some(target) = &await_info.result_target {
        let value = ctx.promise_read(child);
        if target != "__return" && !value.ty.is_void() {
            if let Some(target_field) = fields.get(target) {
                async_store_field(ctx, "%frame", target_field, &value);
            }
        }
    }
    let resume_block = await_resume_block(cfg, await_info.state)?;
    ctx.terminate(format!("  br label %{}", cfg_block_label(resume_block)));
    ctx.emit_label(&exit_label);
    ctx.terminate("  ret void".to_string());
    Ok(())
}

fn frame_field<'a>(
    fields: &'a HashMap<String, FrameField>,
    name: &str,
) -> Result<&'a FrameField, String> {
    fields
        .get(name)
        .ok_or_else(|| format!("Async frame is missing field {}", name))
}

fn current_promise(ctx: &FunctionCtx<'_>) -> Result<String, String> {
    ctx.promise_ptr
        .clone()
        .ok_or_else(|| "Async lowering is missing current promise pointer".to_string())
}

fn emit_async_cfg_op(
    ctx: &mut FunctionCtx<'_>,
    fields: &HashMap<String, FrameField>,
    op: &AsyncOp,
) -> Result<(), String> {
    match op {
        AsyncOp::Decl(ty, decls) => {
            for decl in decls {
                let name = var_decl_name(&decl.var);
                let constants = ctx.visible_constants();
                let value_ty = decl_type_with_constants(ty, &decl.var, ctx.module, &constants)?;
                if matches!(ty, Type::Const(_)) && matches!(value_ty, LlvmType::I32) {
                    if let Some(init) = &decl.init {
                        if let Some(value) = init_const_with_ctx(init, ctx) {
                            ctx.insert_const(name, value);
                        }
                    }
                    continue;
                }
                let ptr = if let Some(field) = fields.get(&name) {
                    async_field_ptr(ctx, "%frame", field)
                } else {
                    let ptr = ctx.alloca(&value_ty);
                    ctx.insert_var(
                        name.clone(),
                        VarInfo {
                            ptr: ptr.clone(),
                            ty: value_ty.clone(),
                        },
                    )?;
                    ptr
                };
                if let Some(init) = &decl.init {
                    if let InitVal::Exp(exp) = unspan_init(init) {
                        let value = emit_exp(ctx, exp)?;
                        store_value_to_ptr(ctx, &value, &value_ty, &ptr);
                    } else {
                        init_store(ctx, init, &value_ty, &ptr)?;
                    }
                }
            }
            Ok(())
        }
        AsyncOp::Assign(lhs, rhs) => {
            let value = emit_exp(ctx, rhs)?;
            let lvalue = emit_lvalue(ctx, lhs)?;
            store_value_to_ptr(ctx, &value, &lvalue.ty, &lvalue.ptr);
            Ok(())
        }
        AsyncOp::Eval(exp) => {
            let _ = emit_exp(ctx, exp)?;
            Ok(())
        }
        AsyncOp::PromiseWait(exp) => {
            let promise = emit_exp(ctx, exp)?;
            let _ = ctx.promise_wait(promise);
            Ok(())
        }
    }
}

fn emit_async_cfg_terminator(
    ctx: &mut FunctionCtx<'_>,
    fields: &HashMap<String, FrameField>,
    cfg: &AsyncCfgFunction,
    awaits: &[AwaitPointInfo],
    ret_ty: &LlvmType,
    term: &AsyncTerminator,
) -> Result<(), String> {
    match term {
        AsyncTerminator::Return(Some(exp)) => {
            let value = emit_exp(ctx, exp)?;
            let promise = current_promise(ctx)?;
            if value.ty.is_void() {
                ctx.promise_resolve(&promise, None);
            } else {
                ctx.promise_resolve_typed(&promise, Some(&value), ret_ty);
            }
            ctx.terminate("  ret void".to_string());
            Ok(())
        }
        AsyncTerminator::Return(None) => {
            let promise = current_promise(ctx)?;
            if ret_ty.is_void() {
                ctx.promise_resolve(&promise, None);
            } else {
                let value = ctx.default_value(ret_ty)?;
                ctx.promise_resolve(&promise, Some(&value));
            }
            ctx.terminate("  ret void".to_string());
            Ok(())
        }
        AsyncTerminator::Jump(target) => {
            ctx.terminate(format!("  br label %{}", cfg_block_label(*target)));
            Ok(())
        }
        AsyncTerminator::Branch {
            cond,
            then_block,
            else_block,
        } => emit_cond_br(
            ctx,
            cond,
            &cfg_block_label(*then_block),
            &cfg_block_label(*else_block),
        ),
        AsyncTerminator::Await(await_term) => {
            emit_async_cfg_await(ctx, fields, cfg, awaits, await_term)
        }
        AsyncTerminator::Unreachable => {
            ctx.terminate("  unreachable".to_string());
            Ok(())
        }
    }
}

fn emit_async_cfg_await(
    ctx: &mut FunctionCtx<'_>,
    fields: &HashMap<String, FrameField>,
    _cfg: &AsyncCfgFunction,
    awaits: &[AwaitPointInfo],
    await_term: &AwaitTerminator,
) -> Result<(), String> {
    let await_info = awaits
        .iter()
        .find(|info| info.state == await_term.state)
        .ok_or_else(|| format!("unknown await state {}", await_term.state))?;
    let child = emit_exp(ctx, &await_term.child)?;
    async_store_field(
        ctx,
        "%frame",
        frame_field(fields, &await_info.child_field)?,
        &child,
    );
    let ready = ctx.tmp();
    ctx.emit(format!(
        "  {} = call i32 @__sysy_promise_is_ready(ptr {})",
        ready, child.name
    ));
    let ready_bool = ctx.tmp();
    ctx.emit(format!("  {} = icmp ne i32 {}, 0", ready_bool, ready));
    let ready_label = ctx.label("async.await.ready");
    let suspend_label = ctx.label("async.await.suspend");
    ctx.terminate(format!(
        "  br i1 {}, label %{}, label %{}",
        ready_bool, ready_label, suspend_label
    ));
    ctx.emit_label(&suspend_label);
    ctx.emit(format!(
        "  call void @__sysy_promise_set_callback(ptr {}, ptr @{}, ptr %frame)",
        child.name, await_info.callback_name
    ));
    ctx.terminate("  br label %async.suspend".to_string());
    ctx.emit_label(&ready_label);
    if let Some(target) = &await_info.result_target {
        if target == "__return" {
            let value = ctx.promise_read(child);
            let promise = current_promise(ctx)?;
            if value.ty.is_void() {
                ctx.promise_resolve(&promise, None);
            } else {
                ctx.promise_resolve(&promise, Some(&value));
            }
            ctx.terminate("  ret void".to_string());
            return Ok(());
        } else {
            let value = ctx.promise_read(child);
            if !value.ty.is_void() {
                if let Some(field) = fields.get(target) {
                    async_store_field(ctx, "%frame", field, &value);
                }
            }
        }
    }
    ctx.terminate(format!(
        "  br label %{}",
        cfg_block_label(await_term.resume_block)
    ));
    Ok(())
}

fn await_resume_block(cfg: &AsyncCfgFunction, state: i32) -> Result<usize, String> {
    cfg.await_points()
        .into_iter()
        .find(|await_term| await_term.state == state)
        .map(|await_term| await_term.resume_block)
        .ok_or_else(|| format!("Async CFG is missing await state {}", state))
}

fn cfg_block_label(block: usize) -> String {
    format!("cfg.block.{}", block)
}

fn init_const_with_ctx(init: &InitVal, ctx: &FunctionCtx<'_>) -> Option<i32> {
    match init {
        InitVal::Spanned(init, _) => init_const_with_ctx(init, ctx),
        InitVal::Exp(exp) => eval_const_exp_with(exp, &|name| ctx.lookup_const(name)),
        InitVal::Arr(_) => None,
    }
}

fn unspan_init(init: &InitVal) -> &InitVal {
    match init {
        InitVal::Spanned(init, _) => unspan_init(init),
        other => other,
    }
}

pub(crate) fn emit_async_main_driver(module: &ModuleCtx, _hidden: &FuncDef) -> String {
    let mut ctx = FunctionCtx::new(module, LlvmType::I32, false);
    let mut out = String::new();
    out.push_str("define i32 @main() {\n");
    ctx.emit_label("entry");
    let promise = ctx.tmp();
    ctx.emit(format!("  {} = call ptr @__sysy_async_main()", promise));
    let call = Value {
        name: promise,
        ty: LlvmType::Promise(Box::new(type_to_llvm(&_hidden.func_type))),
    };
    let waited = ctx.promise_wait(call);
    if matches!(waited.ty, LlvmType::Void) {
        ctx.terminate("  ret i32 0".to_string());
    } else {
        ctx.terminate(format!("  ret i32 {}", waited.name));
    }
    for line in ctx.lines {
        let _ = writeln!(out, "{}", line);
    }
    out.push_str("}\n\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lalr::{BType, BinaryOp};

    #[test]
    fn frame_layout_uses_live_across_await_only() {
        let module = ModuleCtx::new();
        let func = FuncDef {
            span: None,
            is_async: true,
            func_type: Type::BType(BType::I32),
            ident: "keep_live".to_string(),
            func_params: Vec::new(),
            block: vec![
                decl("live", 21),
                decl("dead", 7),
                Stmt::Exp(Exp::Await(Box::new(Exp::Sleep(Box::new(Exp::Number(1)))))),
                Stmt::Return(Some(Exp::BinaryExp(
                    BinaryOp::Mul,
                    Box::new(Exp::Ident("live".to_string())),
                    Box::new(Exp::Number(2)),
                ))),
            ],
        };
        let lowered = rename_async_locals(&func);
        let cfg = build_async_cfg(&lowered);
        let frame_locals = cfg
            .live_across_awaits()
            .into_iter()
            .flat_map(|live| live.vars)
            .collect::<HashSet<_>>();
        let layout = async_frame_layout(
            &module,
            "%async.frame.keep_live",
            &[],
            &lowered.block,
            &frame_locals,
            &HashMap::new(),
            cfg.await_points().len(),
        )
        .expect("frame layout should be valid");

        assert!(layout.fields.keys().any(|name| name.ends_with("_live")));
        assert!(!layout.fields.keys().any(|name| name.ends_with("_dead")));
    }

    #[test]
    fn local_renaming_distinguishes_shadowed_names() {
        let func = FuncDef {
            span: None,
            is_async: true,
            func_type: Type::BType(BType::I32),
            ident: "shadow".to_string(),
            func_params: Vec::new(),
            block: vec![
                decl("x", 30),
                Stmt::Block(vec![
                    decl("x", 5),
                    Stmt::Return(Some(Exp::Ident("x".to_string()))),
                ]),
            ],
        };
        let lowered = rename_async_locals(&func);
        let mut names = Vec::new();
        collect_decl_names(&lowered.block, &mut names);

        assert_eq!(names.len(), 2);
        assert_ne!(names[0], names[1]);
        assert!(names.iter().all(|name| name.starts_with("__sysy_local_")));
    }

    fn decl(name: &str, value: i32) -> Stmt {
        Stmt::Decl(
            Type::BType(BType::I32),
            vec![SingleDecl {
                span: None,
                var: VarDecl::Ident(name.to_string()),
                init: Some(InitVal::Exp(Exp::Number(value))),
            }],
        )
    }

    fn collect_decl_names(stmts: &[Stmt], names: &mut Vec<String>) {
        for stmt in stmts {
            match stmt {
                Stmt::Spanned(stmt, _) => {
                    collect_decl_names(std::slice::from_ref(stmt.as_ref()), names)
                }
                Stmt::Block(stmts) => collect_decl_names(stmts, names),
                Stmt::Decl(_, decls) => {
                    for decl in decls {
                        names.push(var_decl_name(&decl.var));
                    }
                }
                Stmt::If(_, then_stmt) => {
                    collect_decl_names(std::slice::from_ref(then_stmt), names)
                }
                Stmt::IfElse(_, then_stmt, else_stmt) => {
                    collect_decl_names(std::slice::from_ref(then_stmt), names);
                    collect_decl_names(std::slice::from_ref(else_stmt), names);
                }
                Stmt::While(_, body) => collect_decl_names(std::slice::from_ref(body), names),
                Stmt::Assign(_, _)
                | Stmt::Exp(_)
                | Stmt::PromiseWait(_)
                | Stmt::Continue
                | Stmt::Break
                | Stmt::Return(_)
                | Stmt::Empty => {}
            }
        }
    }
}
