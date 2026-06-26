use super::builder::{FunctionCtx, LValue, LoopLabels, Value, VarInfo};
use super::layout::TARGET_LAYOUT;
use super::module::{
    decl_type, decl_type_with_constants, init_const, param_resolved_btype, type_to_llvm,
    var_decl_name, ModuleCtx,
};
use super::names::sanitize_ident;
use super::types::LlvmType;
use crate::lalr::{
    eval_const_exp_with, BinaryOp, Exp, FuncDef, InitVal, SingleDecl, Stmt, Type, UnaryOp,
};
use std::fmt::Write;

pub(crate) fn emit_sync_function(module: &ModuleCtx, func: &FuncDef) -> Result<String, String> {
    let original_ret = type_to_llvm(&func.func_type);
    let llvm_ret = original_ret.clone();
    let params = func
        .func_params
        .iter()
        .map(|param| {
            param_resolved_btype(param, module)
                .map(|ty| (param.name.clone(), LlvmType::from_btype(&ty)))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut ctx = FunctionCtx::new(module, original_ret.clone(), false);
    let param_sig = params
        .iter()
        .enumerate()
        .map(|(idx, (_, ty))| format!("{} %arg{}", ty.llvm(), idx))
        .collect::<Vec<_>>()
        .join(", ");
    let mut out = String::new();
    let _ = writeln!(
        out,
        "define {} @{}({}) {{",
        llvm_ret.llvm(),
        sanitize_ident(&func.ident),
        param_sig
    );
    ctx.emit_label("entry");
    for (idx, (name, ty)) in params.iter().enumerate() {
        let ptr = ctx.alloca(ty);
        ctx.emit(format!("  store {} %arg{}, ptr {}", ty.llvm(), idx, ptr));
        ctx.insert_var(
            name.clone(),
            VarInfo {
                ptr,
                ty: ty.clone(),
            },
        )?;
    }
    emit_stmts(&mut ctx, &func.block)?;
    if !ctx.current_terminated {
        if original_ret.is_void() {
            ctx.terminate("  ret void".to_string());
        } else {
            let value = ctx.default_value(&original_ret)?;
            ctx.terminate(format!("  ret {} {}", original_ret.llvm(), value.name));
        }
    }
    for line in ctx.lines {
        let _ = writeln!(out, "{}", line);
    }
    out.push_str("}\n\n");
    Ok(out)
}

pub(crate) fn emit_global_decl(
    out: &mut String,
    module: &ModuleCtx,
    ty: &Type,
    decls: &[SingleDecl],
) -> Result<(), String> {
    for decl in decls {
        let name = var_decl_name(&decl.var);
        let value_ty = decl_type(ty, &decl.var, module)?;
        if module.constants.contains_key(&name) {
            continue;
        }
        let init = match (&value_ty, &decl.init) {
            (LlvmType::I32, Some(init)) => init_const(init, module)
                .ok_or_else(|| format!("Global initializer for {} must be constant", name))?
                .to_string(),
            (LlvmType::I32, None) => "0".to_string(),
            (_, Some(init)) => const_initializer(init, &value_ty, module)
                .ok_or_else(|| format!("Global initializer for {} must be constant", name))?,
            _ => zero_initializer(&value_ty),
        };
        let _ = writeln!(
            out,
            "@{} = global {} {}",
            sanitize_ident(&name),
            value_ty.llvm(),
            init
        );
    }
    if !decls.is_empty() {
        out.push('\n');
    }
    Ok(())
}

fn emit_stmts(ctx: &mut FunctionCtx<'_>, stmts: &[Stmt]) -> Result<(), String> {
    ctx.push_scope();
    for stmt in stmts {
        if ctx.current_terminated {
            break;
        }
        emit_stmt(ctx, stmt)?;
    }
    ctx.pop_scope();
    Ok(())
}

fn emit_stmt(ctx: &mut FunctionCtx<'_>, stmt: &Stmt) -> Result<(), String> {
    match stmt {
        Stmt::Spanned(stmt, _) => emit_stmt(ctx, stmt),
        Stmt::Block(stmts) => emit_stmts(ctx, stmts),
        Stmt::Assign(lhs, rhs) => {
            let value = emit_exp(ctx, rhs)?;
            let lvalue = emit_lvalue(ctx, lhs)?;
            store_value_to_ptr(ctx, &value, &lvalue.ty, &lvalue.ptr);
            Ok(())
        }
        Stmt::Decl(ty, decls) => {
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
                let ptr = ctx.alloca(&value_ty);
                ctx.insert_var(
                    name,
                    VarInfo {
                        ptr: ptr.clone(),
                        ty: value_ty.clone(),
                    },
                )?;
                if let Some(init) = &decl.init {
                    init_store(ctx, init, &value_ty, &ptr)?;
                }
            }
            Ok(())
        }
        Stmt::Exp(exp) => {
            let _ = emit_exp(ctx, exp)?;
            Ok(())
        }
        Stmt::PromiseWait(exp) => {
            let promise = emit_exp(ctx, exp)?;
            let _ = ctx.promise_wait(promise);
            Ok(())
        }
        Stmt::Return(Some(exp)) => {
            let mut value = emit_exp(ctx, exp)?;
            if ctx.is_async {
                let promise = ctx.promise_ptr.clone().ok_or_else(|| {
                    "Async lowering is missing current promise pointer".to_string()
                })?;
                if value.ty.is_void() {
                    ctx.promise_resolve(&promise, None);
                } else {
                    let ret_ty = ctx.ret_ty.clone();
                    ctx.promise_resolve_typed(&promise, Some(&value), &ret_ty);
                }
                ctx.terminate(format!("  ret ptr {}", promise));
            } else {
                if matches!(ctx.ret_ty, LlvmType::Struct(_))
                    && matches!(&value.ty, LlvmType::Ptr(inner) if **inner == ctx.ret_ty)
                {
                    value = ctx.load(&value.name, &ctx.ret_ty.clone());
                }
                ctx.terminate(format!("  ret {} {}", value.ty.llvm(), value.name));
            }
            Ok(())
        }
        Stmt::Return(None) => {
            if ctx.is_async {
                let promise = ctx.promise_ptr.clone().ok_or_else(|| {
                    "Async lowering is missing current promise pointer".to_string()
                })?;
                ctx.promise_resolve(&promise, None);
                ctx.terminate(format!("  ret ptr {}", promise));
            } else {
                ctx.terminate("  ret void".to_string());
            }
            Ok(())
        }
        Stmt::If(cond, then_stmt) => {
            let then_label = ctx.label("if.then");
            let end_label = ctx.label("if.end");
            emit_cond_br(ctx, cond, &then_label, &end_label)?;
            ctx.emit_label(&then_label);
            emit_stmt(ctx, then_stmt)?;
            if !ctx.current_terminated {
                ctx.terminate(format!("  br label %{}", end_label));
            }
            ctx.emit_label(&end_label);
            Ok(())
        }
        Stmt::IfElse(cond, then_stmt, else_stmt) => {
            let then_label = ctx.label("if.then");
            let else_label = ctx.label("if.else");
            let end_label = ctx.label("if.end");
            emit_cond_br(ctx, cond, &then_label, &else_label)?;
            ctx.emit_label(&then_label);
            emit_stmt(ctx, then_stmt)?;
            if !ctx.current_terminated {
                ctx.terminate(format!("  br label %{}", end_label));
            }
            ctx.emit_label(&else_label);
            emit_stmt(ctx, else_stmt)?;
            if !ctx.current_terminated {
                ctx.terminate(format!("  br label %{}", end_label));
            }
            ctx.emit_label(&end_label);
            Ok(())
        }
        Stmt::While(cond, body) => {
            let cond_label = ctx.label("while.cond");
            let body_label = ctx.label("while.body");
            let end_label = ctx.label("while.end");
            ctx.terminate(format!("  br label %{}", cond_label));
            ctx.emit_label(&cond_label);
            emit_cond_br(ctx, cond, &body_label, &end_label)?;
            ctx.emit_label(&body_label);
            ctx.loop_stack.push(LoopLabels {
                break_label: end_label.clone(),
                continue_label: cond_label.clone(),
            });
            emit_stmt(ctx, body)?;
            ctx.loop_stack.pop();
            if !ctx.current_terminated {
                ctx.terminate(format!("  br label %{}", cond_label));
            }
            ctx.emit_label(&end_label);
            Ok(())
        }
        Stmt::Break => {
            let labels = ctx
                .loop_stack
                .last()
                .ok_or_else(|| "break used outside a loop".to_string())?;
            ctx.terminate(format!("  br label %{}", labels.break_label));
            Ok(())
        }
        Stmt::Continue => {
            let labels = ctx
                .loop_stack
                .last()
                .ok_or_else(|| "continue used outside a loop".to_string())?;
            ctx.terminate(format!("  br label %{}", labels.continue_label));
            Ok(())
        }
        Stmt::Empty => Ok(()),
    }
}

pub(crate) fn emit_exp(ctx: &mut FunctionCtx<'_>, exp: &Exp) -> Result<Value, String> {
    match exp {
        Exp::Spanned(exp, _) => emit_exp(ctx, exp),
        Exp::Number(value) => Ok(Value {
            name: value.to_string(),
            ty: LlvmType::I32,
        }),
        Exp::Ident(name) => {
            if let Some(value) = ctx.lookup_const(name) {
                return Ok(Value {
                    name: value.to_string(),
                    ty: LlvmType::I32,
                });
            }
            if let Some(var) = ctx.lookup_var(name).cloned() {
                Ok(match var.ty {
                    LlvmType::Struct(_) | LlvmType::Array(_, _) => Value {
                        name: var.ptr,
                        ty: LlvmType::Ptr(Box::new(var.ty)),
                    },
                    _ => ctx.load(&var.ptr, &var.ty),
                })
            } else if let Some(global_ty) = ctx.module.globals.get(name).cloned() {
                Ok(match global_ty {
                    LlvmType::Struct(_) | LlvmType::Array(_, _) => Value {
                        name: format!("@{}", sanitize_ident(name)),
                        ty: LlvmType::Ptr(Box::new(global_ty)),
                    },
                    _ => ctx.load(&format!("@{}", sanitize_ident(name)), &global_ty),
                })
            } else {
                Err(format!("Unknown identifier {}", name))
            }
        }
        Exp::UnaryExp(UnaryOp::Addr, inner) => {
            let lvalue = emit_lvalue(ctx, inner)?;
            Ok(Value {
                name: lvalue.ptr,
                ty: LlvmType::Ptr(Box::new(lvalue.ty)),
            })
        }
        Exp::UnaryExp(UnaryOp::Deref, inner) => {
            let ptr = emit_exp(ctx, inner)?;
            match ptr.ty {
                LlvmType::Ptr(inner_ty) => Ok(ctx.load(&ptr.name, &inner_ty)),
                other => Err(format!("Cannot dereference {:?}", other)),
            }
        }
        Exp::UnaryExp(op, inner) => {
            let value = emit_exp(ctx, inner)?;
            match op {
                UnaryOp::Pos => Ok(value),
                UnaryOp::Neg => {
                    let out = ctx.tmp();
                    ctx.emit(format!("  {} = sub i32 0, {}", out, value.name));
                    Ok(Value {
                        name: out,
                        ty: LlvmType::I32,
                    })
                }
                UnaryOp::Not => {
                    let cmp = ctx.tmp();
                    let out = ctx.tmp();
                    ctx.emit(format!("  {} = icmp eq i32 {}, 0", cmp, value.name));
                    ctx.emit(format!("  {} = zext i1 {} to i32", out, cmp));
                    Ok(Value {
                        name: out,
                        ty: LlvmType::I32,
                    })
                }
                UnaryOp::Addr | UnaryOp::Deref => Err(
                    "internal lowering error: address/deref reached scalar unary emission"
                        .to_string(),
                ),
            }
        }
        Exp::BinaryExp(op, lhs, rhs) => emit_binary(ctx, op, lhs, rhs),
        Exp::New(ty) => {
            let value_ty = LlvmType::from_btype(ty);
            let ptr = ctx.tmp();
            ctx.emit(format!(
                "  {} = call ptr @malloc({} {})",
                ptr,
                TARGET_LAYOUT.malloc_size_type,
                value_ty.try_size(&ctx.module.layouts)?
            ));
            Ok(Value {
                name: ptr,
                ty: LlvmType::Ptr(Box::new(value_ty)),
            })
        }
        Exp::Await(inner) => {
            let promise = emit_exp(ctx, inner)?;
            Ok(ctx.promise_wait(promise))
        }
        Exp::Sleep(duration) => {
            let value = emit_exp(ctx, duration)?;
            let promise = ctx.tmp();
            ctx.emit(format!(
                "  {} = call ptr @__sysy_sleep(i32 {})",
                promise, value.name
            ));
            Ok(Value {
                name: promise,
                ty: LlvmType::Promise(Box::new(LlvmType::Void)),
            })
        }
        Exp::PromiseWait(inner) => {
            let promise = emit_exp(ctx, inner)?;
            Ok(ctx.promise_wait(promise))
        }
        Exp::FuncCall(name, args) => {
            let mut values = Vec::new();
            for arg in args {
                values.push(emit_exp(ctx, arg)?);
            }
            call_function(ctx, name, &values)
        }
        Exp::ArrGet(_, _) | Exp::Field(_, _) | Exp::PtrField(_, _) => {
            let lvalue = emit_lvalue(ctx, exp)?;
            match lvalue.ty {
                LlvmType::Struct(_) | LlvmType::Array(_, _) => Ok(Value {
                    name: lvalue.ptr,
                    ty: LlvmType::Ptr(Box::new(lvalue.ty)),
                }),
                _ => Ok(ctx.load(&lvalue.ptr, &lvalue.ty)),
            }
        }
    }
}

fn emit_binary(
    ctx: &mut FunctionCtx<'_>,
    op: &BinaryOp,
    lhs: &Exp,
    rhs: &Exp,
) -> Result<Value, String> {
    if matches!(op, BinaryOp::And | BinaryOp::Or) {
        return emit_short_circuit(ctx, op, lhs, rhs);
    }

    let l = emit_exp(ctx, lhs)?;
    let r = emit_exp(ctx, rhs)?;
    let out = ctx.tmp();
    match op {
        BinaryOp::Add => ctx.emit(format!("  {} = add i32 {}, {}", out, l.name, r.name)),
        BinaryOp::Sub => ctx.emit(format!("  {} = sub i32 {}, {}", out, l.name, r.name)),
        BinaryOp::Mul => ctx.emit(format!("  {} = mul i32 {}, {}", out, l.name, r.name)),
        BinaryOp::Div => ctx.emit(format!("  {} = sdiv i32 {}, {}", out, l.name, r.name)),
        BinaryOp::Mod => ctx.emit(format!("  {} = srem i32 {}, {}", out, l.name, r.name)),
        BinaryOp::And | BinaryOp::Or => {
            return Err(
                "internal lowering error: short-circuit op reached scalar binary emission"
                    .to_string(),
            )
        }
        cmp => {
            let pred =
                match cmp {
                    BinaryOp::Lt => "slt",
                    BinaryOp::Gt => "sgt",
                    BinaryOp::Le => "sle",
                    BinaryOp::Ge => "sge",
                    BinaryOp::Eq => "eq",
                    BinaryOp::Ne => "ne",
                    _ => return Err(
                        "internal lowering error: non-comparison op reached comparison emission"
                            .to_string(),
                    ),
                };
            let cmp_tmp = ctx.tmp();
            ctx.emit(format!(
                "  {} = icmp {} i32 {}, {}",
                cmp_tmp, pred, l.name, r.name
            ));
            ctx.emit(format!("  {} = zext i1 {} to i32", out, cmp_tmp));
        }
    }
    Ok(Value {
        name: out,
        ty: LlvmType::I32,
    })
}

fn emit_short_circuit(
    ctx: &mut FunctionCtx<'_>,
    op: &BinaryOp,
    lhs: &Exp,
    rhs: &Exp,
) -> Result<Value, String> {
    let lhs_value = emit_exp(ctx, lhs)?;
    let lhs_bool = ctx.tmp();
    ctx.emit(format!(
        "  {} = icmp ne i32 {}, 0",
        lhs_bool, lhs_value.name
    ));
    let lhs_label = ctx.current_label.clone();
    let rhs_label = ctx.label("logic.rhs");
    let end_label = ctx.label("logic.end");

    match op {
        BinaryOp::And => ctx.terminate(format!(
            "  br i1 {}, label %{}, label %{}",
            lhs_bool, rhs_label, end_label
        )),
        BinaryOp::Or => ctx.terminate(format!(
            "  br i1 {}, label %{}, label %{}",
            lhs_bool, end_label, rhs_label
        )),
        _ => {
            return Err(
                "internal lowering error: non-short-circuit op reached short-circuit emission"
                    .to_string(),
            )
        }
    }

    ctx.emit_label(&rhs_label);
    let rhs_value = emit_exp(ctx, rhs)?;
    let rhs_bool = ctx.tmp();
    ctx.emit(format!(
        "  {} = icmp ne i32 {}, 0",
        rhs_bool, rhs_value.name
    ));
    let rhs_pred = ctx.current_label.clone();
    ctx.terminate(format!("  br label %{}", end_label));

    ctx.emit_label(&end_label);
    let phi = ctx.tmp();
    let short_value = match op {
        BinaryOp::And => "false",
        BinaryOp::Or => "true",
        _ => {
            return Err(
                "internal lowering error: non-short-circuit op reached short-circuit phi"
                    .to_string(),
            )
        }
    };
    ctx.emit(format!(
        "  {} = phi i1 [{}, %{}], [{}, %{}]",
        phi, short_value, lhs_label, rhs_bool, rhs_pred
    ));
    let out = ctx.tmp();
    ctx.emit(format!("  {} = zext i1 {} to i32", out, phi));
    Ok(Value {
        name: out,
        ty: LlvmType::I32,
    })
}

pub(crate) fn emit_lvalue(ctx: &mut FunctionCtx<'_>, exp: &Exp) -> Result<LValue, String> {
    match exp {
        Exp::Spanned(exp, _) => emit_lvalue(ctx, exp),
        Exp::Ident(name) => {
            if let Some(var) = ctx.lookup_var(name).cloned() {
                Ok(LValue {
                    ptr: var.ptr,
                    ty: var.ty,
                })
            } else if let Some(ty) = ctx.module.globals.get(name).cloned() {
                Ok(LValue {
                    ptr: format!("@{}", sanitize_ident(name)),
                    ty,
                })
            } else {
                Err(format!("Unknown lvalue {}", name))
            }
        }
        Exp::UnaryExp(UnaryOp::Deref, inner) => {
            let ptr = emit_exp(ctx, inner)?;
            match ptr.ty {
                LlvmType::Ptr(inner_ty) => Ok(LValue {
                    ptr: ptr.name,
                    ty: *inner_ty,
                }),
                other => Err(format!("Cannot dereference lvalue {:?}", other)),
            }
        }
        Exp::ArrGet(base, index) => {
            let base_lv = emit_lvalue(ctx, base)?;
            let idx = emit_exp(ctx, index)?;
            let elem_ty = match &base_lv.ty {
                LlvmType::Array(_, inner) => (**inner).clone(),
                LlvmType::Ptr(inner) => (**inner).clone(),
                other => return Err(format!("Cannot index into {:?}", other)),
            };
            let gep = ctx.tmp();
            match &base_lv.ty {
                LlvmType::Array(_, _) => ctx.emit(format!(
                    "  {} = getelementptr inbounds {}, ptr {}, i32 0, i32 {}",
                    gep,
                    base_lv.ty.llvm(),
                    base_lv.ptr,
                    idx.name
                )),
                LlvmType::Ptr(_) => {
                    let loaded_base = ctx.load(&base_lv.ptr, &base_lv.ty);
                    ctx.emit(format!(
                        "  {} = getelementptr inbounds {}, ptr {}, i32 {}",
                        gep,
                        elem_ty.llvm(),
                        loaded_base.name,
                        idx.name
                    ));
                }
                other => {
                    return Err(format!(
                        "internal lowering error: unexpected indexed lvalue type {:?}",
                        other
                    ))
                }
            }
            Ok(LValue {
                ptr: gep,
                ty: elem_ty,
            })
        }
        Exp::Field(base, field_name) => {
            let base_lv = emit_lvalue(ctx, base)?;
            let struct_name = match &base_lv.ty {
                LlvmType::Struct(name) => name.clone(),
                other => return Err(format!("Cannot access field on {:?}", other)),
            };
            let field = ctx.module.field(&struct_name, field_name)?;
            let gep = ctx.tmp();
            ctx.emit(format!(
                "  {} = getelementptr inbounds {}, ptr {}, i32 0, i32 {}",
                gep,
                LlvmType::Struct(struct_name).llvm(),
                base_lv.ptr,
                field.index
            ));
            Ok(LValue {
                ptr: gep,
                ty: field.ty.clone(),
            })
        }
        Exp::PtrField(base, field_name) => {
            let base_value = emit_exp(ctx, base)?;
            let struct_name = match &base_value.ty {
                LlvmType::Ptr(inner) => match &**inner {
                    LlvmType::Struct(name) => name.clone(),
                    other => {
                        return Err(format!("Cannot access pointer field through {:?}", other))
                    }
                },
                other => return Err(format!("Cannot access pointer field through {:?}", other)),
            };
            let field = ctx.module.field(&struct_name, field_name)?;
            let gep = ctx.tmp();
            ctx.emit(format!(
                "  {} = getelementptr inbounds {}, ptr {}, i32 0, i32 {}",
                gep,
                LlvmType::Struct(struct_name).llvm(),
                base_value.name,
                field.index
            ));
            Ok(LValue {
                ptr: gep,
                ty: field.ty.clone(),
            })
        }
        other => Err(format!("Expression {:?} is not assignable", other)),
    }
}

pub(crate) fn emit_cond_br(
    ctx: &mut FunctionCtx<'_>,
    cond: &Exp,
    then_label: &str,
    else_label: &str,
) -> Result<(), String> {
    let value = emit_exp(ctx, cond)?;
    let cmp = ctx.tmp();
    ctx.emit(format!("  {} = icmp ne i32 {}, 0", cmp, value.name));
    ctx.terminate(format!(
        "  br i1 {}, label %{}, label %{}",
        cmp, then_label, else_label
    ));
    Ok(())
}

fn call_function(ctx: &mut FunctionCtx<'_>, name: &str, args: &[Value]) -> Result<Value, String> {
    let sig = ctx
        .module
        .funcs
        .get(name)
        .ok_or_else(|| format!("Unknown function {}", name))?
        .clone();
    let ret_ty = if sig.is_async {
        LlvmType::Promise(Box::new(sig.ret.clone()))
    } else {
        sig.ret.clone()
    };
    if args.len() != sig.params.len() {
        return Err(format!(
            "Function {} expects {} argument(s), got {}",
            name,
            sig.params.len(),
            args.len()
        ));
    }
    let adapted_args = args
        .iter()
        .zip(sig.params.iter())
        .map(|(arg, expected)| adapt_call_arg(ctx, arg, expected))
        .collect::<Vec<_>>();
    let args_text = adapted_args
        .iter()
        .map(|arg| format!("{} {}", arg.ty.llvm(), arg.name))
        .collect::<Vec<_>>()
        .join(", ");
    if ret_ty.is_void() {
        ctx.emit(format!(
            "  call void @{}({})",
            sanitize_ident(name),
            args_text
        ));
        Ok(Value {
            name: "0".to_string(),
            ty: LlvmType::Void,
        })
    } else {
        let out = ctx.tmp();
        ctx.emit(format!(
            "  {} = call {} @{}({})",
            out,
            ret_ty.llvm(),
            sanitize_ident(name),
            args_text
        ));
        Ok(Value {
            name: out,
            ty: ret_ty,
        })
    }
}

fn adapt_call_arg(ctx: &mut FunctionCtx<'_>, arg: &Value, expected: &LlvmType) -> Value {
    match (expected, &arg.ty) {
        (LlvmType::Struct(_), LlvmType::Ptr(inner)) if **inner == *expected => {
            ctx.load(&arg.name, expected)
        }
        (LlvmType::Array(_, _), LlvmType::Ptr(inner)) if **inner == *expected => Value {
            name: arg.name.clone(),
            ty: arg.ty.clone(),
        },
        (LlvmType::Ptr(_), LlvmType::Ptr(_)) => arg.clone(),
        _ => arg.clone(),
    }
}

pub(crate) fn init_store(
    ctx: &mut FunctionCtx<'_>,
    init: &InitVal,
    ty: &LlvmType,
    ptr: &str,
) -> Result<(), String> {
    match init {
        InitVal::Spanned(init, _) => init_store(ctx, init, ty, ptr),
        InitVal::Exp(exp) => {
            let value = emit_exp(ctx, exp)?;
            store_value_to_ptr(ctx, &value, ty, ptr);
            Ok(())
        }
        InitVal::Arr(items) => {
            if let LlvmType::Struct(struct_name) = ty {
                ctx.emit(format!(
                    "  store {} {}, ptr {}",
                    ty.llvm(),
                    zero_initializer(ty),
                    ptr
                ));
                let fields = ctx
                    .module
                    .layouts
                    .get(struct_name)
                    .map(|layout| layout.fields.clone())
                    .unwrap_or_default();
                for (idx, item) in items.iter().enumerate() {
                    let Some(field) = fields.get(idx) else {
                        break;
                    };
                    let field_ptr = ctx.tmp();
                    ctx.emit(format!(
                        "  {} = getelementptr inbounds {}, ptr {}, i32 0, i32 {}",
                        field_ptr,
                        ty.llvm(),
                        ptr,
                        field.index
                    ));
                    init_store(ctx, item, &field.ty, &field_ptr)?;
                }
            } else if let LlvmType::Array(_, _) = ty {
                ctx.emit(format!(
                    "  store {} {}, ptr {}",
                    ty.llvm(),
                    zero_initializer(ty),
                    ptr
                ));
                let scalar_inits = flatten_runtime_array_init(items, ty);
                for (flat_index, exp) in scalar_inits {
                    let scalar_ptr = scalar_ptr_at(ctx, ty, ptr, flat_index);
                    let value = emit_exp(ctx, &exp)?;
                    store_value_to_ptr(ctx, &value, inner_scalar_type(ty), &scalar_ptr);
                }
            }
            Ok(())
        }
    }
}

fn flatten_runtime_array_init(items: &[InitVal], ty: &LlvmType) -> Vec<(usize, Exp)> {
    let mut out = Vec::new();
    flatten_runtime_items(items, ty, 0, &mut out);
    out
}

fn flatten_runtime_items(
    items: &[InitVal],
    ty: &LlvmType,
    base: usize,
    out: &mut Vec<(usize, Exp)>,
) {
    if let LlvmType::Array(len, inner) = ty {
        let inner_count = scalar_count(inner);
        let total = len * inner_count;
        let mut cursor = 0usize;
        for item in items {
            if cursor >= total {
                break;
            }
            let item = unspan_init(item);
            if let InitVal::Exp(exp) = item {
                out.push((base + cursor, exp.clone()));
                cursor += 1;
            } else if let InitVal::Arr(nested) = item {
                flatten_runtime_items(nested, inner, base + cursor, out);
                cursor += inner_count;
            }
        }
    } else if let Some(first) = items.first() {
        if let InitVal::Exp(exp) = unspan_init(first) {
            out.push((base, exp.clone()));
        }
    }
}

fn scalar_count(ty: &LlvmType) -> usize {
    match ty {
        LlvmType::Array(len, inner) => len * scalar_count(inner),
        _ => 1,
    }
}

fn inner_scalar_type(ty: &LlvmType) -> &LlvmType {
    match ty {
        LlvmType::Array(_, inner) => inner_scalar_type(inner),
        other => other,
    }
}

fn scalar_ptr_at(ctx: &mut FunctionCtx<'_>, ty: &LlvmType, ptr: &str, flat_index: usize) -> String {
    match ty {
        LlvmType::Array(_, inner) => {
            let inner_count = scalar_count(inner);
            let elem_index = flat_index / inner_count;
            let rest = flat_index % inner_count;
            let elem_ptr = ctx.tmp();
            ctx.emit(format!(
                "  {} = getelementptr inbounds {}, ptr {}, i32 0, i32 {}",
                elem_ptr,
                ty.llvm(),
                ptr,
                elem_index
            ));
            scalar_ptr_at(ctx, inner, &elem_ptr, rest)
        }
        _ => ptr.to_string(),
    }
}

pub(crate) fn store_value_to_ptr(
    ctx: &mut FunctionCtx<'_>,
    value: &Value,
    dest_ty: &LlvmType,
    ptr: &str,
) {
    match (dest_ty, &value.ty) {
        (LlvmType::Struct(_), LlvmType::Ptr(inner)) if **inner == *dest_ty => {
            let loaded = ctx.load(&value.name, dest_ty);
            ctx.store(&loaded, ptr);
        }
        _ => ctx.store(value, ptr),
    }
}

fn zero_initializer(ty: &LlvmType) -> String {
    match ty {
        LlvmType::I32 => "0".to_string(),
        LlvmType::Void => "zeroinitializer".to_string(),
        LlvmType::Ptr(_) | LlvmType::Promise(_) => "null".to_string(),
        LlvmType::Struct(_) | LlvmType::Array(_, _) => "zeroinitializer".to_string(),
    }
}

fn const_initializer(init: &InitVal, ty: &LlvmType, module: &ModuleCtx) -> Option<String> {
    match (init, ty) {
        (InitVal::Spanned(init, _), _) => const_initializer(init, ty, module),
        (InitVal::Exp(exp), LlvmType::I32) => {
            Some(eval_const_exp_with(exp, &|name| module.constants.get(name).copied())?.to_string())
        }
        (InitVal::Exp(_), _) => None,
        (InitVal::Arr(items), LlvmType::Struct(struct_name)) => {
            let layout = module.layouts.get(struct_name)?;
            let mut values = Vec::new();
            for (idx, field) in layout.fields.iter().enumerate() {
                let value = if let Some(item) = items.get(idx) {
                    const_initializer(item, &field.ty, module)?
                } else {
                    zero_initializer(&field.ty)
                };
                values.push(format!("{} {}", field.ty.llvm(), value));
            }
            Some(format!("{{ {} }}", values.join(", ")))
        }
        (InitVal::Arr(items), LlvmType::Array(_, _)) => {
            let mut flat = vec!["0".to_string(); scalar_count(ty)];
            flatten_const_items(items, ty, 0, &mut flat, module)?;
            let mut cursor = 0usize;
            Some(const_initializer_from_flat(ty, &flat, &mut cursor))
        }
        _ => None,
    }
}

fn init_const_with_ctx(init: &InitVal, ctx: &FunctionCtx<'_>) -> Option<i32> {
    match init {
        InitVal::Spanned(init, _) => init_const_with_ctx(init, ctx),
        InitVal::Exp(exp) => eval_const_exp_with(exp, &|name| ctx.lookup_const(name)),
        InitVal::Arr(_) => None,
    }
}

fn flatten_const_items(
    items: &[InitVal],
    ty: &LlvmType,
    base: usize,
    flat: &mut [String],
    module: &ModuleCtx,
) -> Option<()> {
    if let LlvmType::Array(len, inner) = ty {
        let inner_count = scalar_count(inner);
        let total = len * inner_count;
        let mut cursor = 0usize;
        for item in items {
            if cursor >= total {
                break;
            }
            let item = unspan_init(item);
            if let InitVal::Exp(exp) = item {
                flat[base + cursor] =
                    eval_const_exp_with(exp, &|name| module.constants.get(name).copied())?
                        .to_string();
                cursor += 1;
            } else if let InitVal::Arr(nested) = item {
                flatten_const_items(nested, inner, base + cursor, flat, module)?;
                cursor += inner_count;
            }
        }
        Some(())
    } else if let Some(first) = items.first() {
        if let InitVal::Exp(exp) = unspan_init(first) {
            flat[base] =
                eval_const_exp_with(exp, &|name| module.constants.get(name).copied())?.to_string();
        }
        Some(())
    } else {
        Some(())
    }
}

fn unspan_init(init: &InitVal) -> &InitVal {
    match init {
        InitVal::Spanned(init, _) => unspan_init(init),
        other => other,
    }
}

fn const_initializer_from_flat(ty: &LlvmType, flat: &[String], cursor: &mut usize) -> String {
    match ty {
        LlvmType::Array(len, inner) => {
            let mut values = Vec::new();
            for _ in 0..*len {
                let value = const_initializer_from_flat(inner, flat, cursor);
                values.push(format!("{} {}", inner.llvm(), value));
            }
            format!("[{}]", values.join(", "))
        }
        _ => {
            let value = flat
                .get(*cursor)
                .cloned()
                .unwrap_or_else(|| zero_initializer(ty));
            *cursor += 1;
            value
        }
    }
}
