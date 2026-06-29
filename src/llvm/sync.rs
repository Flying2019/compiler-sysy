use super::builder::{FunctionCtx, LValue, LoopLabels, Value, VarInfo};
use super::codegen::IrModule;
use super::module::{
    decl_type, decl_type_with_constants, param_resolved_btype, type_to_llvm, var_decl_name,
    ModuleCtx,
};
use super::types::LlvmType;
use crate::lalr::{
    eval_const_exp_with, BinaryOp, Exp, FuncDef, InitVal, SingleDecl, Stmt, Type, UnaryOp,
};
use inkwell::values::{BasicValueEnum, IntValue, PointerValue};
use inkwell::IntPredicate;

pub(crate) fn emit_sync_function<'ctx>(
    ir: &IrModule<'ctx>,
    module: &ModuleCtx,
    func: &FuncDef,
) -> Result<(), String> {
    let original_ret = type_to_llvm(&func.func_type);
    let params = func
        .func_params
        .iter()
        .map(|param| {
            param_resolved_btype(param, module)
                .map(|ty| (param.name.clone(), LlvmType::from_btype(&ty)))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let function = ir.function(&func.ident)?;
    let mut ctx = FunctionCtx::new(ir, module, function, original_ret.clone(), false);
    for (idx, (name, ty)) in params.iter().enumerate() {
        let param = function
            .get_nth_param(idx as u32)
            .ok_or_else(|| format!("Function {} is missing parameter {}", func.ident, idx))?;
        let ptr = ctx.alloca(ty)?;
        ctx.store(&Value::from_basic(param, ty.clone()), ptr)?;
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
            ctx.terminate_return(None)?;
        } else {
            let value = ctx.default_value(&original_ret)?;
            ctx.terminate_return(Some(&value))?;
        }
    }
    Ok(())
}

pub(crate) fn emit_global_decl<'ctx>(
    ir: &mut IrModule<'ctx>,
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
        let init = match &decl.init {
            Some(init) => const_initializer(ir, init, &value_ty, module)
                .ok_or_else(|| format!("Global initializer for {} must be constant", name))?,
            None => zero_initializer(ir, &value_ty)?,
        };
        ir.add_global(&name, &value_ty, init)?;
    }
    Ok(())
}

fn emit_stmts<'ctx>(ctx: &mut FunctionCtx<'_, 'ctx>, stmts: &[Stmt]) -> Result<(), String> {
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

fn emit_stmt<'ctx>(ctx: &mut FunctionCtx<'_, 'ctx>, stmt: &Stmt) -> Result<(), String> {
    match stmt {
        Stmt::Spanned(stmt, _) => emit_stmt(ctx, stmt),
        Stmt::Block(stmts) => emit_stmts(ctx, stmts),
        Stmt::Assign(lhs, rhs) => {
            let value = emit_exp(ctx, rhs)?;
            let lvalue = emit_lvalue(ctx, lhs)?;
            store_value_to_ptr(ctx, &value, &lvalue.ty, lvalue.ptr)
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
                let ptr = ctx.alloca(&value_ty)?;
                ctx.insert_var(
                    name,
                    VarInfo {
                        ptr,
                        ty: value_ty.clone(),
                    },
                )?;
                if let Some(init) = &decl.init {
                    init_store(ctx, init, &value_ty, ptr)?;
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
            let _ = ctx.promise_wait(promise)?;
            Ok(())
        }
        Stmt::Return(Some(exp)) => {
            let mut value = emit_exp(ctx, exp)?;
            if ctx.is_async {
                let promise = ctx.promise_ptr.ok_or_else(|| {
                    "Async lowering is missing current promise pointer".to_string()
                })?;
                if value.ty.is_void() {
                    ctx.promise_resolve(promise, None)?;
                } else {
                    let ret_ty = ctx.ret_ty.clone();
                    ctx.promise_resolve_typed(promise, Some(&value), &ret_ty)?;
                }
                ctx.terminate_return(None)?;
            } else {
                if matches!(ctx.ret_ty, LlvmType::Struct(_))
                    && matches!(&value.ty, LlvmType::Ptr(inner) if **inner == ctx.ret_ty)
                {
                    value = ctx.load(value.ptr_value()?, &ctx.ret_ty.clone())?;
                }
                ctx.terminate_return(Some(&value))?;
            }
            Ok(())
        }
        Stmt::Return(None) => {
            if ctx.is_async {
                let promise = ctx.promise_ptr.ok_or_else(|| {
                    "Async lowering is missing current promise pointer".to_string()
                })?;
                ctx.promise_resolve(promise, None)?;
            }
            ctx.terminate_return(None)
        }
        Stmt::If(cond, then_stmt) => {
            let then_label = ctx.label("if.then");
            let end_label = ctx.label("if.end");
            emit_cond_br(ctx, cond, &then_label, &end_label)?;
            ctx.emit_label(&then_label);
            emit_stmt(ctx, then_stmt)?;
            if !ctx.current_terminated {
                ctx.terminate_br(&end_label)?;
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
                ctx.terminate_br(&end_label)?;
            }
            ctx.emit_label(&else_label);
            emit_stmt(ctx, else_stmt)?;
            if !ctx.current_terminated {
                ctx.terminate_br(&end_label)?;
            }
            ctx.emit_label(&end_label);
            Ok(())
        }
        Stmt::While(cond, body) => {
            let cond_label = ctx.label("while.cond");
            let body_label = ctx.label("while.body");
            let end_label = ctx.label("while.end");
            ctx.terminate_br(&cond_label)?;
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
                ctx.terminate_br(&cond_label)?;
            }
            ctx.emit_label(&end_label);
            Ok(())
        }
        Stmt::Break => {
            let labels = ctx
                .loop_stack
                .last()
                .cloned()
                .ok_or_else(|| "break used outside a loop".to_string())?;
            ctx.terminate_br(&labels.break_label)
        }
        Stmt::Continue => {
            let labels = ctx
                .loop_stack
                .last()
                .cloned()
                .ok_or_else(|| "continue used outside a loop".to_string())?;
            ctx.terminate_br(&labels.continue_label)
        }
        Stmt::Empty => Ok(()),
    }
}

pub(crate) fn emit_exp<'ctx>(
    ctx: &mut FunctionCtx<'_, 'ctx>,
    exp: &Exp,
) -> Result<Value<'ctx>, String> {
    match exp {
        Exp::Spanned(exp, _) => emit_exp(ctx, exp),
        Exp::Number(value) => Ok(ctx.int_const(*value)),
        Exp::Ident(name) => {
            if let Some(value) = ctx.lookup_const(name) {
                return Ok(ctx.int_const(value));
            }
            if let Some(var) = ctx.lookup_var(name).cloned() {
                Ok(match var.ty {
                    LlvmType::Struct(_) | LlvmType::Array(_, _) => {
                        Value::from_basic(var.ptr.into(), LlvmType::Ptr(Box::new(var.ty)))
                    }
                    _ => ctx.load(var.ptr, &var.ty)?,
                })
            } else if let Some(global_ty) = ctx.module.globals.get(name).cloned() {
                let global = ctx.ir.global(name)?.as_pointer_value();
                Ok(match global_ty {
                    LlvmType::Struct(_) | LlvmType::Array(_, _) => {
                        Value::from_basic(global.into(), LlvmType::Ptr(Box::new(global_ty)))
                    }
                    _ => ctx.load(global, &global_ty)?,
                })
            } else {
                Err(format!("Unknown identifier {}", name))
            }
        }
        Exp::UnaryExp(UnaryOp::Addr, inner) => {
            let lvalue = emit_lvalue(ctx, inner)?;
            Ok(Value::from_basic(
                lvalue.ptr.into(),
                LlvmType::Ptr(Box::new(lvalue.ty)),
            ))
        }
        Exp::UnaryExp(UnaryOp::Deref, inner) => {
            let ptr = emit_exp(ctx, inner)?;
            match &ptr.ty {
                LlvmType::Ptr(inner_ty) => ctx.load(ptr.ptr_value()?, inner_ty),
                other => Err(format!("Cannot dereference {:?}", other)),
            }
        }
        Exp::UnaryExp(op, inner) => {
            let value = emit_exp(ctx, inner)?;
            match op {
                UnaryOp::Pos => Ok(value),
                UnaryOp::Neg => {
                    let name = ctx.tmp();
                    let out = ctx
                        .builder
                        .build_int_neg(value.int_value()?, &name)
                        .map_err(|err| err.to_string())?;
                    Ok(Value::from_basic(out.into(), LlvmType::I32))
                }
                UnaryOp::Not => {
                    let cmp = ctx.i32_ne_zero(&value)?;
                    let name = ctx.tmp();
                    let not = ctx
                        .builder
                        .build_not(cmp, &name)
                        .map_err(|err| err.to_string())?;
                    let out = zext_bool(ctx, not)?;
                    Ok(Value::from_basic(out.into(), LlvmType::I32))
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
            let size = value_ty.try_size(&ctx.module.layouts)? as u64;
            let malloc_ret = ctx.call_named_typed(
                "malloc",
                &[ctx.i64_const(size)],
                LlvmType::Ptr(Box::new(value_ty.clone())),
            )?;
            Ok(malloc_ret)
        }
        Exp::Await(inner) | Exp::PromiseWait(inner) => {
            let promise = emit_exp(ctx, inner)?;
            ctx.promise_wait(promise)
        }
        Exp::Sleep(duration) => {
            let value = emit_exp(ctx, duration)?;
            ctx.call_named_typed(
                "__sysy_sleep",
                &[value],
                LlvmType::Promise(Box::new(LlvmType::Void)),
            )
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
                LlvmType::Struct(_) | LlvmType::Array(_, _) => Ok(Value::from_basic(
                    lvalue.ptr.into(),
                    LlvmType::Ptr(Box::new(lvalue.ty)),
                )),
                _ => ctx.load(lvalue.ptr, &lvalue.ty),
            }
        }
    }
}

fn emit_binary<'ctx>(
    ctx: &mut FunctionCtx<'_, 'ctx>,
    op: &BinaryOp,
    lhs: &Exp,
    rhs: &Exp,
) -> Result<Value<'ctx>, String> {
    if matches!(op, BinaryOp::And | BinaryOp::Or) {
        return emit_short_circuit(ctx, op, lhs, rhs);
    }
    let l = emit_exp(ctx, lhs)?.int_value()?;
    let r = emit_exp(ctx, rhs)?.int_value()?;
    let name = ctx.tmp();
    let out = match op {
        BinaryOp::Add => ctx.builder.build_int_add(l, r, &name),
        BinaryOp::Sub => ctx.builder.build_int_sub(l, r, &name),
        BinaryOp::Mul => ctx.builder.build_int_mul(l, r, &name),
        BinaryOp::Div => ctx.builder.build_int_signed_div(l, r, &name),
        BinaryOp::Mod => ctx.builder.build_int_signed_rem(l, r, &name),
        BinaryOp::And | BinaryOp::Or => {
            return Err(
                "internal lowering error: short-circuit op reached scalar binary emission"
                    .to_string(),
            )
        }
        cmp => {
            let pred =
                match cmp {
                    BinaryOp::Lt => IntPredicate::SLT,
                    BinaryOp::Gt => IntPredicate::SGT,
                    BinaryOp::Le => IntPredicate::SLE,
                    BinaryOp::Ge => IntPredicate::SGE,
                    BinaryOp::Eq => IntPredicate::EQ,
                    BinaryOp::Ne => IntPredicate::NE,
                    _ => return Err(
                        "internal lowering error: non-comparison op reached comparison emission"
                            .to_string(),
                    ),
                };
            let cmp_value = ctx
                .builder
                .build_int_compare(pred, l, r, &name)
                .map_err(|err| err.to_string())?;
            let out = zext_bool(ctx, cmp_value)?;
            return Ok(Value::from_basic(out.into(), LlvmType::I32));
        }
    }
    .map_err(|err| err.to_string())?;
    Ok(Value::from_basic(out.into(), LlvmType::I32))
}

fn emit_short_circuit<'ctx>(
    ctx: &mut FunctionCtx<'_, 'ctx>,
    op: &BinaryOp,
    lhs: &Exp,
    rhs: &Exp,
) -> Result<Value<'ctx>, String> {
    let lhs_value = emit_exp(ctx, lhs)?;
    let lhs_bool = ctx.i32_ne_zero(&lhs_value)?;
    let lhs_block = ctx.current_block;
    let rhs_label = ctx.label("logic.rhs");
    let end_label = ctx.label("logic.end");
    match op {
        BinaryOp::And => ctx.terminate_cond_br(lhs_bool, &rhs_label, &end_label)?,
        BinaryOp::Or => ctx.terminate_cond_br(lhs_bool, &end_label, &rhs_label)?,
        _ => {
            return Err(
                "internal lowering error: non-short-circuit op reached short-circuit emission"
                    .to_string(),
            )
        }
    }
    ctx.emit_label(&rhs_label);
    let rhs_value = emit_exp(ctx, rhs)?;
    let rhs_bool = ctx.i32_ne_zero(&rhs_value)?;
    let rhs_block = ctx.current_block;
    ctx.terminate_br(&end_label)?;

    ctx.emit_label(&end_label);
    let phi_name = ctx.tmp();
    let phi = ctx
        .builder
        .build_phi(ctx.ir.bool_type(), &phi_name)
        .map_err(|err| err.to_string())?;
    let short_value = match op {
        BinaryOp::And => ctx.ir.bool_type().const_zero(),
        BinaryOp::Or => ctx.ir.bool_type().const_int(1, false),
        _ => unreachable!(),
    };
    phi.add_incoming(&[(&short_value, lhs_block), (&rhs_bool, rhs_block)]);
    let out = zext_bool(ctx, phi.as_basic_value().into_int_value())?;
    Ok(Value::from_basic(out.into(), LlvmType::I32))
}

pub(crate) fn emit_lvalue<'ctx>(
    ctx: &mut FunctionCtx<'_, 'ctx>,
    exp: &Exp,
) -> Result<LValue<'ctx>, String> {
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
                    ptr: ctx.ir.global(name)?.as_pointer_value(),
                    ty,
                })
            } else {
                Err(format!("Unknown lvalue {}", name))
            }
        }
        Exp::UnaryExp(UnaryOp::Deref, inner) => {
            let ptr = emit_exp(ctx, inner)?;
            match &ptr.ty {
                LlvmType::Ptr(inner_ty) => Ok(LValue {
                    ptr: ptr.ptr_value()?,
                    ty: (**inner_ty).clone(),
                }),
                other => Err(format!("Cannot dereference lvalue {:?}", other)),
            }
        }
        Exp::ArrGet(base, index) => {
            let base_lv = emit_lvalue(ctx, base)?;
            let idx = emit_exp(ctx, index)?.int_value()?;
            let elem_ty = match &base_lv.ty {
                LlvmType::Array(_, inner) => (**inner).clone(),
                LlvmType::Ptr(inner) => (**inner).clone(),
                other => return Err(format!("Cannot index into {:?}", other)),
            };
            let gep = match &base_lv.ty {
                LlvmType::Array(_, _) => {
                    let zero = ctx.ir.i32_type().const_zero();
                    let name = ctx.tmp();
                    unsafe {
                        ctx.builder
                            .build_in_bounds_gep(
                                ctx.ir.basic_type(&base_lv.ty)?,
                                base_lv.ptr,
                                &[zero, idx],
                                &name,
                            )
                            .map_err(|err| err.to_string())?
                    }
                }
                LlvmType::Ptr(_) => {
                    let loaded_base = ctx.load(base_lv.ptr, &base_lv.ty)?.ptr_value()?;
                    let name = ctx.tmp();
                    unsafe {
                        ctx.builder
                            .build_in_bounds_gep(
                                ctx.ir.basic_type(&elem_ty)?,
                                loaded_base,
                                &[idx],
                                &name,
                            )
                            .map_err(|err| err.to_string())?
                    }
                }
                other => {
                    return Err(format!(
                        "internal lowering error: unexpected indexed lvalue type {:?}",
                        other
                    ))
                }
            };
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
            let name = ctx.tmp();
            let gep = ctx
                .builder
                .build_struct_gep(
                    ctx.ir.struct_type(&struct_name)?,
                    base_lv.ptr,
                    field.index as u32,
                    &name,
                )
                .map_err(|err| err.to_string())?;
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
            let name = ctx.tmp();
            let gep = ctx
                .builder
                .build_struct_gep(
                    ctx.ir.struct_type(&struct_name)?,
                    base_value.ptr_value()?,
                    field.index as u32,
                    &name,
                )
                .map_err(|err| err.to_string())?;
            Ok(LValue {
                ptr: gep,
                ty: field.ty.clone(),
            })
        }
        other => Err(format!("Expression {:?} is not assignable", other)),
    }
}

pub(crate) fn emit_cond_br<'ctx>(
    ctx: &mut FunctionCtx<'_, 'ctx>,
    cond: &Exp,
    then_label: &str,
    else_label: &str,
) -> Result<(), String> {
    let value = emit_exp(ctx, cond)?;
    let cmp = ctx.i32_ne_zero(&value)?;
    ctx.terminate_cond_br(cmp, then_label, else_label)
}

fn call_function<'ctx>(
    ctx: &mut FunctionCtx<'_, 'ctx>,
    name: &str,
    args: &[Value<'ctx>],
) -> Result<Value<'ctx>, String> {
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
        .collect::<Result<Vec<_>, _>>()?;
    if ret_ty.is_void() {
        ctx.call_named_void(name, &adapted_args)?;
        Ok(Value {
            value: None,
            ty: LlvmType::Void,
        })
    } else {
        ctx.call_named_typed(name, &adapted_args, ret_ty)
    }
}

fn adapt_call_arg<'ctx>(
    ctx: &mut FunctionCtx<'_, 'ctx>,
    arg: &Value<'ctx>,
    expected: &LlvmType,
) -> Result<Value<'ctx>, String> {
    match (expected, &arg.ty) {
        (LlvmType::Struct(_), LlvmType::Ptr(inner)) if **inner == *expected => {
            ctx.load(arg.ptr_value()?, expected)
        }
        (LlvmType::Array(_, _), LlvmType::Ptr(inner)) if **inner == *expected => Ok(arg.clone()),
        (LlvmType::Ptr(_), LlvmType::Ptr(_)) => Ok(arg.clone()),
        _ => Ok(arg.clone()),
    }
}

pub(crate) fn init_store<'ctx>(
    ctx: &mut FunctionCtx<'_, 'ctx>,
    init: &InitVal,
    ty: &LlvmType,
    ptr: PointerValue<'ctx>,
) -> Result<(), String> {
    match init {
        InitVal::Spanned(init, _) => init_store(ctx, init, ty, ptr),
        InitVal::Exp(exp) => {
            let value = emit_exp(ctx, exp)?;
            store_value_to_ptr(ctx, &value, ty, ptr)
        }
        InitVal::Arr(items) => {
            let default = ctx.default_value(ty)?;
            ctx.store(&default, ptr)?;
            if let LlvmType::Struct(struct_name) = ty {
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
                    let name = ctx.tmp();
                    let field_ptr = ctx
                        .builder
                        .build_struct_gep(
                            ctx.ir.struct_type(struct_name)?,
                            ptr,
                            field.index as u32,
                            &name,
                        )
                        .map_err(|err| err.to_string())?;
                    init_store(ctx, item, &field.ty, field_ptr)?;
                }
            } else if let LlvmType::Array(_, _) = ty {
                let scalar_inits = flatten_runtime_array_init(items, ty);
                for (flat_index, exp) in scalar_inits {
                    let scalar_ptr = scalar_ptr_at(ctx, ty, ptr, flat_index)?;
                    let value = emit_exp(ctx, &exp)?;
                    store_value_to_ptr(ctx, &value, inner_scalar_type(ty), scalar_ptr)?;
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

fn scalar_ptr_at<'ctx>(
    ctx: &mut FunctionCtx<'_, 'ctx>,
    ty: &LlvmType,
    ptr: PointerValue<'ctx>,
    flat_index: usize,
) -> Result<PointerValue<'ctx>, String> {
    match ty {
        LlvmType::Array(_, inner) => {
            let inner_count = scalar_count(inner);
            let elem_index = flat_index / inner_count;
            let rest = flat_index % inner_count;
            let zero = ctx.ir.i32_type().const_zero();
            let idx = ctx.ir.i32_type().const_int(elem_index as u64, false);
            let name = ctx.tmp();
            let elem_ptr = unsafe {
                ctx.builder
                    .build_in_bounds_gep(ctx.ir.basic_type(ty)?, ptr, &[zero, idx], &name)
                    .map_err(|err| err.to_string())?
            };
            scalar_ptr_at(ctx, inner, elem_ptr, rest)
        }
        _ => Ok(ptr),
    }
}

pub(crate) fn store_value_to_ptr<'ctx>(
    ctx: &mut FunctionCtx<'_, 'ctx>,
    value: &Value<'ctx>,
    dest_ty: &LlvmType,
    ptr: PointerValue<'ctx>,
) -> Result<(), String> {
    match (dest_ty, &value.ty) {
        (LlvmType::Struct(_), LlvmType::Ptr(inner)) if **inner == *dest_ty => {
            let loaded = ctx.load(value.ptr_value()?, dest_ty)?;
            ctx.store(&loaded, ptr)
        }
        _ => ctx.store(value, ptr),
    }
}

fn zext_bool<'ctx>(
    ctx: &mut FunctionCtx<'_, 'ctx>,
    value: IntValue<'ctx>,
) -> Result<IntValue<'ctx>, String> {
    let name = ctx.tmp();
    ctx.builder
        .build_int_z_extend(value, ctx.ir.i32_type(), &name)
        .map_err(|err| err.to_string())
}

fn zero_initializer<'ctx>(
    ir: &IrModule<'ctx>,
    ty: &LlvmType,
) -> Result<BasicValueEnum<'ctx>, String> {
    Ok(ir.basic_type(ty)?.const_zero())
}

fn const_initializer<'ctx>(
    ir: &IrModule<'ctx>,
    init: &InitVal,
    ty: &LlvmType,
    module: &ModuleCtx,
) -> Option<BasicValueEnum<'ctx>> {
    match (init, ty) {
        (InitVal::Spanned(init, _), _) => const_initializer(ir, init, ty, module),
        (InitVal::Exp(exp), LlvmType::I32) => Some(
            ir.i32_type()
                .const_int(
                    eval_const_exp_with(exp, &|name| module.constants.get(name).copied())? as u64,
                    true,
                )
                .into(),
        ),
        (InitVal::Exp(_), _) => None,
        (InitVal::Arr(items), LlvmType::Struct(struct_name)) => {
            let layout = module.layouts.get(struct_name)?;
            let mut values = Vec::new();
            for (idx, field) in layout.fields.iter().enumerate() {
                let value = if let Some(item) = items.get(idx) {
                    const_initializer(ir, item, &field.ty, module)?
                } else {
                    zero_initializer(ir, &field.ty).ok()?
                };
                values.push(value);
            }
            Some(
                ir.struct_type(struct_name)
                    .ok()?
                    .const_named_struct(&values)
                    .into(),
            )
        }
        (InitVal::Arr(items), LlvmType::Array(_, _)) => {
            let mut flat = vec![0i32; scalar_count(ty)];
            flatten_const_items(items, ty, 0, &mut flat, module)?;
            let mut cursor = 0usize;
            const_initializer_from_flat(ir, ty, &flat, &mut cursor).ok()
        }
        _ => None,
    }
}

fn init_const_with_ctx(init: &InitVal, ctx: &FunctionCtx<'_, '_>) -> Option<i32> {
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
    flat: &mut [i32],
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
                    eval_const_exp_with(exp, &|name| module.constants.get(name).copied())?;
                cursor += 1;
            } else if let InitVal::Arr(nested) = item {
                flatten_const_items(nested, inner, base + cursor, flat, module)?;
                cursor += inner_count;
            }
        }
        Some(())
    } else if let Some(first) = items.first() {
        if let InitVal::Exp(exp) = unspan_init(first) {
            flat[base] = eval_const_exp_with(exp, &|name| module.constants.get(name).copied())?;
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

fn const_initializer_from_flat<'ctx>(
    ir: &IrModule<'ctx>,
    ty: &LlvmType,
    flat: &[i32],
    cursor: &mut usize,
) -> Result<BasicValueEnum<'ctx>, String> {
    match ty {
        LlvmType::Array(len, inner) => {
            let mut values = Vec::new();
            for _ in 0..*len {
                values.push(const_initializer_from_flat(ir, inner, flat, cursor)?);
            }
            const_array(ir, inner, &values)
        }
        LlvmType::I32 => {
            let value = flat.get(*cursor).copied().unwrap_or_default();
            *cursor += 1;
            Ok(ir.i32_type().const_int(value as u64, true).into())
        }
        _ => {
            *cursor += 1;
            zero_initializer(ir, ty)
        }
    }
}

fn const_array<'ctx>(
    ir: &IrModule<'ctx>,
    elem_ty: &LlvmType,
    values: &[BasicValueEnum<'ctx>],
) -> Result<BasicValueEnum<'ctx>, String> {
    match ir.basic_type(elem_ty)? {
        inkwell::types::BasicTypeEnum::IntType(ty) => Ok(ty
            .const_array(
                &values
                    .iter()
                    .map(|value| value.into_int_value())
                    .collect::<Vec<_>>(),
            )
            .into()),
        inkwell::types::BasicTypeEnum::ArrayType(ty) => Ok(ty
            .const_array(
                &values
                    .iter()
                    .map(|value| value.into_array_value())
                    .collect::<Vec<_>>(),
            )
            .into()),
        inkwell::types::BasicTypeEnum::StructType(ty) => Ok(ty
            .const_array(
                &values
                    .iter()
                    .map(|value| value.into_struct_value())
                    .collect::<Vec<_>>(),
            )
            .into()),
        inkwell::types::BasicTypeEnum::PointerType(ty) => Ok(ty
            .const_array(
                &values
                    .iter()
                    .map(|value| value.into_pointer_value())
                    .collect::<Vec<_>>(),
            )
            .into()),
        other => Err(format!(
            "Unsupported constant array element type {:?}",
            other
        )),
    }
}
