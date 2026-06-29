use crate::lalr::{CompUnit, Diagnostic, FuncDef, GlobalDef};
use crate::llvm::async_lower::{emit_async_function, emit_async_main_driver};
use crate::llvm::codegen::IrModule;
use crate::llvm::module::ModuleCtx;
use crate::llvm::module::{param_resolved_btype, type_to_llvm};
use crate::llvm::sync::{emit_global_decl, emit_sync_function};
use crate::llvm::types::LlvmType;
use inkwell::context::Context;

pub const DEFAULT_RISCV_TARGET: &str = "riscv64-unknown-unknown-elf";

pub fn compile_to_llvm(ast: &CompUnit) -> String {
    compile_to_llvm_with_target(ast, DEFAULT_RISCV_TARGET)
}

pub fn try_compile_to_llvm(ast: &CompUnit) -> Result<String, Diagnostic> {
    try_compile_to_llvm_with_target(ast, DEFAULT_RISCV_TARGET)
}

pub fn compile_to_llvm_with_target(ast: &CompUnit, target_triple: &str) -> String {
    try_compile_to_llvm_with_target(ast, target_triple).unwrap_or_else(|err| panic!("{}", err))
}

pub fn try_compile_to_llvm_with_target(
    ast: &CompUnit,
    target_triple: &str,
) -> Result<String, Diagnostic> {
    ast.validate_semantics()?;
    catch_lowering_errors(|| emit_llvm_unchecked(ast, target_triple))
}

fn catch_lowering_errors<F>(f: F) -> Result<String, Diagnostic>
where
    F: FnOnce() -> Result<String, Diagnostic>,
{
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    std::panic::set_hook(previous_hook);
    result.map_err(|payload| {
        let message = if let Some(message) = payload.downcast_ref::<String>() {
            message.clone()
        } else if let Some(message) = payload.downcast_ref::<&str>() {
            (*message).to_string()
        } else {
            "unknown panic".to_string()
        };
        Diagnostic::new(format!("LLVM lowering failed: {}", message))
    })?
}

fn emit_llvm_unchecked(ast: &CompUnit, target_triple: &str) -> Result<String, Diagnostic> {
    let mut module = ModuleCtx::new();
    module.scan(ast)?;
    module.resolve_struct_layouts()?;

    let context = Context::create();
    let mut ir = IrModule::new(&context, "compile-sysy", target_triple, &module)
        .map_err(Diagnostic::from)?;
    declare_user_functions(&mut ir, &module, ast).map_err(Diagnostic::from)?;

    for glob_def in &ast.global_defs {
        if let GlobalDef::GlobalDecl(ty, decls) = glob_def {
            emit_global_decl(&mut ir, &module, ty, decls).map_err(Diagnostic::from)?;
        }
    }

    for glob_def in &ast.global_defs {
        if let GlobalDef::FuncDef(func) = glob_def {
            if func.is_async && func.ident == "main" {
                let mut hidden = func.clone();
                hidden.ident = "__sysy_async_main".to_string();
                emit_function(&mut ir, &module, &hidden)?;
                emit_async_main_driver(&ir, &module, &hidden).map_err(Diagnostic::from)?;
            } else {
                emit_function(&mut ir, &module, func)?;
            }
        }
    }
    Ok(ir.module.print_to_string().to_string())
}

fn declare_user_functions<'ctx>(
    ir: &mut IrModule<'ctx>,
    module: &ModuleCtx,
    ast: &CompUnit,
) -> Result<(), String> {
    for glob_def in &ast.global_defs {
        let GlobalDef::FuncDef(func) = glob_def else {
            continue;
        };
        let ret = if func.is_async {
            LlvmType::Promise(Box::new(type_to_llvm(&func.func_type)))
        } else {
            type_to_llvm(&func.func_type)
        };
        let params = func
            .func_params
            .iter()
            .map(|param| param_resolved_btype(param, module).map(|ty| LlvmType::from_btype(&ty)))
            .collect::<Result<Vec<_>, _>>()?;
        if func.is_async && func.ident == "main" {
            ir.declare_function("__sysy_async_main", &ret, &params)?;
            ir.declare_function("main", &LlvmType::I32, &[])?;
        } else {
            ir.declare_function(&func.ident, &ret, &params)?;
        }
    }
    Ok(())
}

fn emit_function<'ctx>(
    ir: &mut IrModule<'ctx>,
    module: &ModuleCtx,
    func: &FuncDef,
) -> Result<(), Diagnostic> {
    if func.is_async {
        emit_async_function(ir, module, func).map_err(Diagnostic::from)
    } else {
        emit_sync_function(ir, module, func).map_err(Diagnostic::from)
    }
}
