use crate::lalr::{
    eval_const_exp_with, eval_param_dim, func_param_btype, BType, BinaryOp, CompUnit, Exp, FuncDef,
    FuncParam, GlobalDef, InitVal, SingleDecl, Stmt, StructDef, Type, UnaryOp, VarDecl,
};
use crate::llvm::async_cfg::{
    build_async_cfg, AsyncCfgFunction, AsyncOp, AsyncTerminator, AwaitTerminator,
};
use crate::llvm::layout::{
    align_up, FieldLayout, StructLayout, RISCV64_DATA_LAYOUT, TARGET_LAYOUT,
};
use crate::llvm::types::LlvmType;
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

pub use crate::llvm::toolchain::compile_llvm_to_riscv_asm;

pub const DEFAULT_RISCV_TARGET: &str = "riscv64-unknown-unknown-elf";

#[derive(Debug, Clone)]
struct FuncSig {
    ret: LlvmType,
    is_async: bool,
    params: Vec<LlvmType>,
}

#[derive(Debug, Clone)]
struct VarInfo {
    ptr: String,
    ty: LlvmType,
}

#[derive(Debug, Clone)]
struct Value {
    name: String,
    ty: LlvmType,
}

#[derive(Debug, Clone)]
struct LValue {
    ptr: String,
    ty: LlvmType,
}

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

#[derive(Debug, Default)]
struct ModuleCtx {
    structs: HashMap<String, StructDef>,
    layouts: HashMap<String, StructLayout>,
    funcs: HashMap<String, FuncSig>,
    globals: HashMap<String, LlvmType>,
    constants: HashMap<String, i32>,
}

#[derive(Debug)]
struct FunctionCtx<'a> {
    module: &'a ModuleCtx,
    ret_ty: LlvmType,
    vars: Vec<HashMap<String, VarInfo>>,
    lines: Vec<String>,
    tmp_counter: usize,
    label_counter: usize,
    current_label: String,
    current_terminated: bool,
    loop_stack: Vec<LoopLabels>,
    is_async: bool,
    promise_ptr: Option<String>,
}

#[derive(Debug, Clone)]
struct LoopLabels {
    break_label: String,
    continue_label: String,
}

pub fn compile_to_llvm(ast: &CompUnit) -> String {
    compile_to_llvm_with_target(ast, DEFAULT_RISCV_TARGET)
}

pub fn try_compile_to_llvm(ast: &CompUnit) -> Result<String, String> {
    try_compile_to_llvm_with_target(ast, DEFAULT_RISCV_TARGET)
}

pub fn compile_to_llvm_with_target(ast: &CompUnit, target_triple: &str) -> String {
    try_compile_to_llvm_with_target(ast, target_triple).unwrap_or_else(|err| panic!("{}", err))
}

pub fn try_compile_to_llvm_with_target(
    ast: &CompUnit,
    target_triple: &str,
) -> Result<String, String> {
    let ast = normalize_async_awaits(ast);
    ast.validate_semantics()?;
    catch_lowering_errors(|| emit_llvm_unchecked(&ast, target_triple))
}

fn catch_lowering_errors<F>(f: F) -> Result<String, String>
where
    F: FnOnce() -> String,
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
        format!("LLVM lowering failed: {}", message)
    })
}

fn normalize_async_awaits(ast: &CompUnit) -> CompUnit {
    let mut used_names = HashSet::new();
    collect_names_comp_unit(ast, &mut used_names);
    let mut lifter = AwaitLifter {
        next_id: 0,
        used_names,
    };
    CompUnit {
        global_defs: ast
            .global_defs
            .iter()
            .map(|global| match global {
                GlobalDef::FuncDef(func) if func.is_async => {
                    let mut func = func.clone();
                    func.block = lifter.lift_stmts(&func.block);
                    GlobalDef::FuncDef(func)
                }
                other => other.clone(),
            })
            .collect(),
    }
}

struct AwaitLifter {
    next_id: usize,
    used_names: HashSet<String>,
}

impl AwaitLifter {
    fn lift_stmts(&mut self, stmts: &[Stmt]) -> Vec<Stmt> {
        let mut out = Vec::new();
        for stmt in stmts {
            out.extend(self.lift_stmt(stmt));
        }
        out
    }

    fn lift_stmt(&mut self, stmt: &Stmt) -> Vec<Stmt> {
        let mut prelude = Vec::new();
        let stmt = match stmt {
            Stmt::Block(stmts) => Stmt::Block(self.lift_stmts(stmts)),
            Stmt::Assign(lhs, rhs)
                if matches!(lhs, Exp::Ident(_)) && matches!(rhs, Exp::Await(_)) =>
            {
                Stmt::Assign(lhs.clone(), self.lift_top_level_await(rhs, &mut prelude))
            }
            Stmt::Assign(lhs, rhs) => {
                let rhs = self.lift_exp(rhs, &mut prelude);
                Stmt::Assign(lhs.clone(), rhs)
            }
            Stmt::Decl(ty, decls) => {
                let mut out = Vec::new();
                for decl in decls {
                    let mut decl_prelude = Vec::new();
                    let init = decl.init.as_ref().map(|init| match init {
                        InitVal::Exp(exp) if matches!(exp, Exp::Await(_)) => {
                            InitVal::Exp(self.lift_top_level_await(exp, &mut decl_prelude))
                        }
                        _ => self.lift_init(init, &mut decl_prelude),
                    });
                    out.extend(decl_prelude);
                    out.push(Stmt::Decl(
                        ty.clone(),
                        vec![SingleDecl {
                            var: decl.var.clone(),
                            init,
                        }],
                    ));
                }
                return out;
            }
            Stmt::Exp(exp) if matches!(exp, Exp::Await(_)) => {
                Stmt::Exp(self.lift_top_level_await(exp, &mut prelude))
            }
            Stmt::Exp(exp) => Stmt::Exp(self.lift_exp(exp, &mut prelude)),
            Stmt::Return(Some(exp)) if matches!(exp, Exp::Await(_)) => {
                Stmt::Return(Some(self.lift_top_level_await(exp, &mut prelude)))
            }
            Stmt::Return(Some(exp)) => Stmt::Return(Some(self.lift_exp(exp, &mut prelude))),
            Stmt::PromiseWait(exp) => Stmt::PromiseWait(self.lift_exp(exp, &mut prelude)),
            Stmt::If(cond, then_stmt) => {
                let cond = self.lift_exp(cond, &mut prelude);
                Stmt::If(cond, Box::new(self.lift_stmt_as_block(then_stmt)))
            }
            Stmt::IfElse(cond, then_stmt, else_stmt) => {
                let cond = self.lift_exp(cond, &mut prelude);
                Stmt::IfElse(
                    cond,
                    Box::new(self.lift_stmt_as_block(then_stmt)),
                    Box::new(self.lift_stmt_as_block(else_stmt)),
                )
            }
            Stmt::While(cond, body) => {
                // Await in a loop condition cannot be hoisted without changing loop semantics.
                Stmt::While(cond.clone(), Box::new(self.lift_stmt_as_block(body)))
            }
            Stmt::Continue | Stmt::Break | Stmt::Return(None) | Stmt::Empty => stmt.clone(),
        };
        prelude.push(stmt);
        prelude
    }

    fn lift_stmt_as_block(&mut self, stmt: &Stmt) -> Stmt {
        match stmt {
            Stmt::Block(stmts) => Stmt::Block(self.lift_stmts(stmts)),
            other => {
                let stmts = self.lift_stmt(other);
                if stmts.len() == 1 {
                    stmts.into_iter().next().unwrap()
                } else {
                    Stmt::Block(stmts)
                }
            }
        }
    }

    fn lift_top_level_await(&mut self, exp: &Exp, prelude: &mut Vec<Stmt>) -> Exp {
        match exp {
            Exp::Await(inner) => Exp::Await(Box::new(self.lift_exp(inner, prelude))),
            _ => self.lift_exp(exp, prelude),
        }
    }

    fn lift_init(&mut self, init: &InitVal, prelude: &mut Vec<Stmt>) -> InitVal {
        match init {
            InitVal::Exp(exp) => InitVal::Exp(self.lift_exp(exp, prelude)),
            InitVal::Arr(items) => InitVal::Arr(
                items
                    .iter()
                    .map(|item| self.lift_init(item, prelude))
                    .collect(),
            ),
        }
    }

    fn lift_exp(&mut self, exp: &Exp, prelude: &mut Vec<Stmt>) -> Exp {
        match exp {
            Exp::Await(inner) => {
                let inner = self.lift_exp(inner, prelude);
                let temp = self.next_temp();
                prelude.push(Stmt::Decl(
                    Type::BType(BType::I32),
                    vec![SingleDecl {
                        var: VarDecl::Ident(temp.clone()),
                        init: Some(InitVal::Exp(Exp::Await(Box::new(inner)))),
                    }],
                ));
                Exp::Ident(temp)
            }
            Exp::UnaryExp(op, inner) => {
                Exp::UnaryExp(op.clone(), Box::new(self.lift_exp(inner, prelude)))
            }
            Exp::BinaryExp(op @ (BinaryOp::And | BinaryOp::Or), lhs, rhs) => {
                Exp::BinaryExp(op.clone(), lhs.clone(), rhs.clone())
            }
            Exp::BinaryExp(op, lhs, rhs) => Exp::BinaryExp(
                op.clone(),
                Box::new(self.lift_exp(lhs, prelude)),
                Box::new(self.lift_exp(rhs, prelude)),
            ),
            Exp::Sleep(inner) => Exp::Sleep(Box::new(self.lift_exp(inner, prelude))),
            Exp::PromiseWait(inner) => Exp::PromiseWait(Box::new(self.lift_exp(inner, prelude))),
            Exp::FuncCall(name, args) => Exp::FuncCall(
                name.clone(),
                args.iter().map(|arg| self.lift_exp(arg, prelude)).collect(),
            ),
            Exp::ArrGet(base, index) => Exp::ArrGet(
                Box::new(self.lift_exp(base, prelude)),
                Box::new(self.lift_exp(index, prelude)),
            ),
            Exp::Field(base, field) => {
                Exp::Field(Box::new(self.lift_exp(base, prelude)), field.clone())
            }
            Exp::PtrField(base, field) => {
                Exp::PtrField(Box::new(self.lift_exp(base, prelude)), field.clone())
            }
            Exp::Number(_) | Exp::New(_) | Exp::Ident(_) => exp.clone(),
        }
    }

    fn next_temp(&mut self) -> String {
        loop {
            let name = format!("sysy_await_tmp_{}", self.next_id);
            self.next_id += 1;
            if self.used_names.insert(name.clone()) {
                return name;
            }
        }
    }
}

fn collect_names_comp_unit(ast: &CompUnit, names: &mut HashSet<String>) {
    for global in &ast.global_defs {
        match global {
            GlobalDef::FuncDef(func) => {
                names.insert(func.ident.clone());
                for param in &func.func_params {
                    names.insert(param.name.clone());
                }
                collect_names_stmts(&func.block, names);
            }
            GlobalDef::GlobalDecl(_, decls) => {
                for decl in decls {
                    names.insert(var_decl_name(&decl.var));
                }
            }
            GlobalDef::StructDef(def) => {
                names.insert(def.name.clone());
                for field in &def.fields {
                    for decl in &field.decls {
                        names.insert(var_decl_name(&decl.var));
                    }
                }
            }
        }
    }
}

fn collect_names_stmts(stmts: &[Stmt], names: &mut HashSet<String>) {
    for stmt in stmts {
        match stmt {
            Stmt::Block(stmts) => collect_names_stmts(stmts, names),
            Stmt::Decl(_, decls) => {
                for decl in decls {
                    names.insert(var_decl_name(&decl.var));
                }
            }
            Stmt::If(_, then_stmt) => collect_names_stmt(then_stmt, names),
            Stmt::IfElse(_, then_stmt, else_stmt) => {
                collect_names_stmt(then_stmt, names);
                collect_names_stmt(else_stmt, names);
            }
            Stmt::While(_, body) => collect_names_stmt(body, names),
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

fn collect_names_stmt(stmt: &Stmt, names: &mut HashSet<String>) {
    collect_names_stmts(std::slice::from_ref(stmt), names);
}

fn emit_llvm_unchecked(ast: &CompUnit, target_triple: &str) -> String {
    let mut module = ModuleCtx::new();
    module.scan(ast);
    module.resolve_struct_layouts();

    let mut out = String::new();
    out.push_str("; generated by compile-sysy LLVM backend\n");
    if target_triple.starts_with("riscv64") {
        let _ = writeln!(out, "target datalayout = \"{}\"", RISCV64_DATA_LAYOUT);
    }
    let _ = writeln!(out, "target triple = \"{}\"\n", target_triple);

    for (name, layout) in stable_structs(&module.layouts) {
        let fields = layout
            .fields
            .iter()
            .map(|field| field.ty.llvm())
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            out,
            "%struct.{} = type {{ {} }}",
            sanitize_ident(name),
            fields
        );
    }
    if !module.layouts.is_empty() {
        out.push('\n');
    }

    out.push_str("%promise = type { i32, i32, i64, ptr, ptr, ptr, ptr, ptr, ptr }\n\n");
    let _ = writeln!(
        out,
        "declare ptr @malloc({})",
        TARGET_LAYOUT.malloc_size_type
    );
    out.push_str("declare void @free(ptr)\n");
    out.push_str("declare i32 @getint()\n");
    out.push_str("declare i32 @getch()\n");
    out.push_str("declare i32 @getarray(ptr)\n");
    out.push_str("declare void @putint(i32)\n");
    out.push_str("declare i32 @putch(i32)\n");
    out.push_str("declare void @putarray(i32, ptr)\n");
    out.push_str("declare void @starttime()\n");
    out.push_str("declare void @stoptime()\n\n");
    out.push_str("declare ptr @__sysy_promise_new(i64)\n");
    out.push_str("declare void @__sysy_promise_resolve(ptr, ptr)\n");
    out.push_str("declare ptr @__sysy_promise_result_ptr(ptr)\n");
    out.push_str("declare void @__sysy_promise_wait(ptr)\n");
    out.push_str("declare i32 @__sysy_promise_is_ready(ptr)\n");
    out.push_str("declare void @__sysy_promise_set_callback(ptr, ptr, ptr)\n");
    out.push_str("declare void @__sysy_pending_add(ptr, ptr, ptr)\n");
    out.push_str("declare void @__sysy_promise_clear_driver(ptr)\n");
    out.push_str("declare ptr @__sysy_sleep(i32)\n\n");

    for glob_def in &ast.global_defs {
        if let GlobalDef::GlobalDecl(ty, decls) = glob_def {
            emit_global_decl(&mut out, &module, ty, decls);
        }
    }

    for glob_def in &ast.global_defs {
        if let GlobalDef::FuncDef(func) = glob_def {
            if func.is_async && func.ident == "main" {
                let mut hidden = func.clone();
                hidden.ident = "__sysy_async_main".to_string();
                out.push_str(&emit_function(&module, &hidden));
                out.push_str(&emit_async_main_driver(&module, &hidden));
            } else {
                out.push_str(&emit_function(&module, func));
            }
        }
    }
    out
}

impl ModuleCtx {
    fn new() -> Self {
        Self::default()
    }

    fn scan(&mut self, ast: &CompUnit) {
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
                        .map(|param| LlvmType::from_btype(&param_resolved_btype(param, self)))
                        .collect::<Vec<_>>();
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
                        let value_ty = decl_type(ty, &decl.var, self);
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
    }

    fn insert_runtime_funcs(&mut self) {
        let i32_ty = LlvmType::I32;
        let void_ty = LlvmType::Void;
        let ptr_i32 = LlvmType::Ptr(Box::new(LlvmType::I32));
        for (name, ret, _params) in [
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
                    params: _params,
                },
            );
        }
    }

    fn resolve_struct_layouts(&mut self) {
        let names = self.structs.keys().cloned().collect::<Vec<_>>();
        for name in names {
            self.ensure_struct_layout(&name, &mut Vec::new());
        }
    }

    fn ensure_struct_layout(&mut self, name: &str, visiting: &mut Vec<String>) -> StructLayout {
        if let Some(layout) = self.layouts.get(name) {
            return layout.clone();
        }
        if visiting.iter().any(|item| item == name) {
            panic!("Cyclic struct definition detected: {}", name);
        }
        visiting.push(name.to_string());
        let def = self
            .structs
            .get(name)
            .unwrap_or_else(|| panic!("Unknown struct type {}", name))
            .clone();
        let mut fields = Vec::new();
        let mut offset = 0usize;
        let mut align = 1usize;
        for field in &def.fields {
            for decl in &field.decls {
                let ty = decl_type(&field.ty, &decl.var, self);
                if let LlvmType::Struct(inner) = &ty {
                    self.ensure_struct_layout(inner, visiting);
                }
                let field_align = ty.align(&self.layouts);
                let field_size = ty.size(&self.layouts);
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
        layout
    }

    fn field(&self, struct_name: &str, field_name: &str) -> &FieldLayout {
        self.layouts
            .get(struct_name)
            .unwrap_or_else(|| panic!("Unknown struct type {}", struct_name))
            .fields
            .iter()
            .find(|field| field.name == field_name)
            .unwrap_or_else(|| panic!("Struct {} has no field {}", struct_name, field_name))
    }
}

impl<'a> FunctionCtx<'a> {
    fn new(module: &'a ModuleCtx, ret_ty: LlvmType, is_async: bool) -> Self {
        Self {
            module,
            ret_ty,
            vars: vec![HashMap::new()],
            lines: Vec::new(),
            tmp_counter: 0,
            label_counter: 0,
            current_label: "entry".to_string(),
            current_terminated: false,
            loop_stack: Vec::new(),
            is_async,
            promise_ptr: None,
        }
    }

    fn push_scope(&mut self) {
        self.vars.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.vars.pop();
    }

    fn insert_var(&mut self, name: String, info: VarInfo) {
        self.vars
            .last_mut()
            .expect("No active variable scope")
            .insert(name, info);
    }

    fn lookup_var(&self, name: &str) -> Option<&VarInfo> {
        self.vars.iter().rev().find_map(|scope| scope.get(name))
    }

    fn tmp(&mut self) -> String {
        let name = format!("%t{}", self.tmp_counter);
        self.tmp_counter += 1;
        name
    }

    fn label(&mut self, prefix: &str) -> String {
        let label = format!("{}.{}", prefix, self.label_counter);
        self.label_counter += 1;
        label
    }

    fn emit(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
    }

    fn emit_label(&mut self, label: &str) {
        self.lines.push(format!("{}:", label));
        self.current_label = label.to_string();
        self.current_terminated = false;
    }

    fn terminate(&mut self, line: impl Into<String>) {
        if !self.current_terminated {
            self.emit(line);
            self.current_terminated = true;
        }
    }

    fn alloca(&mut self, ty: &LlvmType) -> String {
        let ptr = self.tmp();
        self.emit(format!("  {} = alloca {}", ptr, ty.llvm()));
        ptr
    }

    fn load(&mut self, ptr: &str, ty: &LlvmType) -> Value {
        let val = self.tmp();
        self.emit(format!("  {} = load {}, ptr {}", val, ty.llvm(), ptr));
        Value {
            name: val,
            ty: ty.clone(),
        }
    }

    fn store(&mut self, value: &Value, ptr: &str) {
        self.emit(format!(
            "  store {} {}, ptr {}",
            value.ty.llvm(),
            value.name,
            ptr
        ));
    }

    fn default_value(&mut self, ty: &LlvmType) -> Value {
        match ty {
            LlvmType::I32 => Value {
                name: "0".to_string(),
                ty: LlvmType::I32,
            },
            LlvmType::Ptr(_) | LlvmType::Promise(_) => Value {
                name: "null".to_string(),
                ty: ty.clone(),
            },
            other => panic!("No scalar default value for {:?}", other),
        }
    }

    fn promise_new(&mut self, value_ty: &LlvmType) -> Value {
        let ptr = self.tmp();
        let size = value_ty.size(&self.module.layouts);
        self.emit(format!(
            "  {} = call ptr @__sysy_promise_new(i64 {})",
            ptr, size
        ));
        Value {
            name: ptr,
            ty: LlvmType::Promise(Box::new(value_ty.clone())),
        }
    }

    fn promise_resolve(&mut self, promise: &str, value: Option<&Value>) {
        match value {
            Some(value) if !value.ty.is_void() => {
                let slot = self.alloca(&value.ty);
                self.store(value, &slot);
                self.emit(format!(
                    "  call void @__sysy_promise_resolve(ptr {}, ptr {})",
                    promise, slot
                ));
            }
            _ => self.emit(format!(
                "  call void @__sysy_promise_resolve(ptr {}, ptr null)",
                promise
            )),
        }
    }

    fn promise_resolve_typed(&mut self, promise: &str, value: Option<&Value>, expected: &LlvmType) {
        match value {
            Some(value) if matches!((&value.ty, expected), (LlvmType::Ptr(inner), expected) if **inner == *expected) =>
            {
                self.emit(format!(
                    "  call void @__sysy_promise_resolve(ptr {}, ptr {})",
                    promise, value.name
                ));
            }
            _ => self.promise_resolve(promise, value),
        }
    }

    fn promise_wait(&mut self, promise: Value) -> Value {
        let value_ty = promise.ty.promise_value();
        self.emit(format!(
            "  call void @__sysy_promise_wait(ptr {})",
            promise.name
        ));
        if value_ty.is_void() {
            Value {
                name: "0".to_string(),
                ty: LlvmType::Void,
            }
        } else {
            self.promise_read(Value {
                name: promise.name,
                ty: LlvmType::Promise(Box::new(value_ty)),
            })
        }
    }

    fn promise_read(&mut self, promise: Value) -> Value {
        let value_ty = promise.ty.promise_value();
        if value_ty.is_void() {
            Value {
                name: "0".to_string(),
                ty: LlvmType::Void,
            }
        } else {
            let result_ptr = self.tmp();
            self.emit(format!(
                "  {} = call ptr @__sysy_promise_result_ptr(ptr {})",
                result_ptr, promise.name
            ));
            self.load(&result_ptr, &value_ty)
        }
    }
}

fn emit_function(module: &ModuleCtx, func: &FuncDef) -> String {
    if func.is_async {
        return emit_async_function(module, func);
    }

    let original_ret = type_to_llvm(&func.func_type);
    let llvm_ret = original_ret.clone();
    let params = func
        .func_params
        .iter()
        .map(|param| {
            (
                param.name.clone(),
                LlvmType::from_btype(&param_resolved_btype(param, module)),
            )
        })
        .collect::<Vec<_>>();
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
        );
    }
    emit_stmts(&mut ctx, &func.block);
    if !ctx.current_terminated {
        if original_ret.is_void() {
            ctx.terminate("  ret void".to_string());
        } else {
            let value = ctx.default_value(&original_ret);
            ctx.terminate(format!("  ret {} {}", original_ret.llvm(), value.name));
        }
    }
    for line in ctx.lines {
        let _ = writeln!(out, "{}", line);
    }
    out.push_str("}\n\n");
    out
}

fn async_frame_layout(
    module: &ModuleCtx,
    frame_type: &str,
    params: &[(String, LlvmType)],
    stmts: &[Stmt],
    frame_locals: &HashSet<String>,
    extra_i32_fields: &HashSet<String>,
    await_count: usize,
) -> AsyncFrameLayout {
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
    );
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
        );
    }
    collect_async_decl_fields(
        module,
        frame_type,
        stmts,
        frame_locals,
        &mut fields,
        &mut ordered_fields,
        &mut offset,
        &mut max_align,
    );
    for name in extra_i32_fields {
        add_frame_field(
            &mut fields,
            &mut ordered_fields,
            &mut offset,
            &mut max_align,
            &module.layouts,
            frame_type,
            name.clone(),
            LlvmType::I32,
        );
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
        );
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
    );
    AsyncFrameLayout {
        type_name: frame_type.to_string(),
        fields,
        ordered_fields,
        size: align_up(offset, max_align),
    }
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
) {
    if fields.contains_key(&name) {
        return;
    }
    let field_align = ty.align(structs);
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
    *offset += ty.size(structs);
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
) {
    for stmt in stmts {
        match stmt {
            Stmt::Decl(ty, decls) => {
                for decl in decls {
                    let name = var_decl_name(&decl.var);
                    if !frame_locals.contains(&name) {
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
                        decl_type(ty, &decl.var, module),
                    );
                }
            }
            Stmt::Block(stmts) => collect_async_decl_fields(
                module,
                frame_type,
                stmts,
                frame_locals,
                fields,
                ordered_fields,
                offset,
                max_align,
            ),
            Stmt::If(_, then_stmt) => collect_async_decl_fields(
                module,
                frame_type,
                std::slice::from_ref(then_stmt),
                frame_locals,
                fields,
                ordered_fields,
                offset,
                max_align,
            ),
            Stmt::IfElse(_, then_stmt, else_stmt) => {
                collect_async_decl_fields(
                    module,
                    frame_type,
                    std::slice::from_ref(then_stmt),
                    frame_locals,
                    fields,
                    ordered_fields,
                    offset,
                    max_align,
                );
                collect_async_decl_fields(
                    module,
                    frame_type,
                    std::slice::from_ref(else_stmt),
                    frame_locals,
                    fields,
                    ordered_fields,
                    offset,
                    max_align,
                );
            }
            Stmt::While(_, body) => collect_async_decl_fields(
                module,
                frame_type,
                std::slice::from_ref(body),
                frame_locals,
                fields,
                ordered_fields,
                offset,
                max_align,
            ),
            _ => {}
        }
    }
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

fn emit_async_function(module: &ModuleCtx, func: &FuncDef) -> String {
    let ret_ty = type_to_llvm(&func.func_type);
    let params = func
        .func_params
        .iter()
        .map(|param| {
            (
                param.name.clone(),
                LlvmType::from_btype(&param_resolved_btype(param, module)),
            )
        })
        .collect::<Vec<_>>();
    let safe_name = sanitize_ident(&func.ident);
    let frame_type = format!("%async.frame.{}", safe_name);
    let cfg = build_async_cfg(func);
    let mut frame_locals = cfg
        .live_across_awaits()
        .into_iter()
        .flat_map(|live| live.vars)
        .collect::<HashSet<_>>();
    let start_name = format!("__sysy_async_start_{}", safe_name);
    let awaits = collect_await_points(module, &safe_name, &cfg);
    let mut extra_i32_fields = HashSet::new();
    for await_info in &awaits {
        if let Some(target) = &await_info.result_target {
            if target != "__return" {
                if target.starts_with("__sysy_cfg_await_tmp_") {
                    extra_i32_fields.insert(target.clone());
                } else {
                    frame_locals.insert(target.clone());
                }
            }
        }
    }
    let mut all_decl_names = HashSet::new();
    collect_names_stmts(&func.block, &mut all_decl_names);
    frame_locals.extend(all_decl_names);
    let frame_layout = async_frame_layout(
        module,
        &frame_type,
        &params,
        &func.block,
        &frame_locals,
        &extra_i32_fields,
        awaits.len(),
    );
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
    let promise = entry.promise_new(&ret_ty);
    let frame = entry.tmp();
    entry.emit(format!(
        "  {} = call ptr @malloc({} {})",
        frame, TARGET_LAYOUT.malloc_size_type, frame_layout.size,
    ));
    async_store_field(
        &mut entry,
        &frame,
        fields.get("__promise").unwrap(),
        &promise,
    );
    for (idx, (name, ty)) in params.iter().enumerate() {
        let value = Value {
            name: format!("%arg{}", idx),
            ty: ty.clone(),
        };
        async_store_field(&mut entry, &frame, fields.get(name).unwrap(), &value);
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
    ));

    for await_info in &awaits {
        out.push_str(&emit_async_cfg_function(
            module,
            &await_info.callback_name,
            &ret_ty,
            fields,
            &cfg,
            &awaits,
            await_resume_block(&cfg, await_info.state).unwrap_or(cfg.entry),
            Some(await_info),
        ));
    }

    out
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
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "define void @{}(ptr %frame) {{", name);
    let mut ctx = FunctionCtx::new(module, LlvmType::Void, true);
    ctx.emit_label("entry");
    init_async_frame_vars(&mut ctx, fields);

    if let Some(await_info) = continuation {
        emit_continuation_entry(&mut ctx, fields, await_info, cfg);
    } else {
        let promise = ctx.promise_ptr.clone().unwrap();
        ctx.emit(format!(
            "  call void @__sysy_promise_clear_driver(ptr {})",
            promise
        ));
        ctx.terminate(format!("  br label %{}", cfg_block_label(entry_block)));
    }

    for block in &cfg.blocks {
        ctx.emit_label(&cfg_block_label(block.id));
        for op in &block.ops {
            emit_async_cfg_op(&mut ctx, fields, op);
        }
        emit_async_cfg_terminator(&mut ctx, fields, cfg, awaits, ret_ty, &block.terminator);
    }

    ctx.emit_label("async.suspend");
    ctx.terminate("  ret void".to_string());
    for line in ctx.lines {
        let _ = writeln!(out, "{}", line);
    }
    out.push_str("}\n\n");
    out
}

fn init_async_frame_vars(ctx: &mut FunctionCtx<'_>, fields: &HashMap<String, FrameField>) {
    let promise = async_load_field(ctx, "%frame", fields.get("__promise").unwrap());
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
        );
    }
}

fn is_internal_async_frame_field(name: &str) -> bool {
    name == "__promise" || name == "__return" || name.starts_with("__await")
}

fn emit_continuation_entry(
    ctx: &mut FunctionCtx<'_>,
    fields: &HashMap<String, FrameField>,
    await_info: &AwaitPointInfo,
    cfg: &AsyncCfgFunction,
) {
    let child = async_load_field(ctx, "%frame", fields.get(&await_info.child_field).unwrap());
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
    let resume_block = await_resume_block(cfg, await_info.state).unwrap_or(cfg.entry);
    ctx.terminate(format!("  br label %{}", cfg_block_label(resume_block)));
    ctx.emit_label(&exit_label);
    ctx.terminate("  ret void".to_string());
}

fn emit_async_cfg_op(
    ctx: &mut FunctionCtx<'_>,
    fields: &HashMap<String, FrameField>,
    op: &AsyncOp,
) {
    match op {
        AsyncOp::Decl(ty, decls) => {
            for decl in decls {
                let name = var_decl_name(&decl.var);
                let value_ty = decl_type(ty, &decl.var, ctx.module);
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
                    );
                    ptr
                };
                if let Some(init) = &decl.init {
                    match init {
                        InitVal::Exp(exp) => {
                            let value = emit_exp(ctx, exp);
                            store_value_to_ptr(ctx, &value, &value_ty, &ptr);
                        }
                        InitVal::Arr(_) => init_store(ctx, init, &value_ty, &ptr),
                    }
                }
            }
        }
        AsyncOp::Assign(lhs, rhs) => {
            let value = emit_exp(ctx, rhs);
            let lvalue = emit_lvalue(ctx, lhs);
            store_value_to_ptr(ctx, &value, &lvalue.ty, &lvalue.ptr);
        }
        AsyncOp::Eval(exp) => {
            let _ = emit_exp(ctx, exp);
        }
        AsyncOp::PromiseWait(exp) => {
            let promise = emit_exp(ctx, exp);
            let _ = ctx.promise_wait(promise);
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
) {
    match term {
        AsyncTerminator::Return(Some(exp)) => {
            let value = emit_exp(ctx, exp);
            let promise = ctx.promise_ptr.clone().unwrap();
            if value.ty.is_void() {
                ctx.promise_resolve(&promise, None);
            } else {
                ctx.promise_resolve_typed(&promise, Some(&value), ret_ty);
            }
            ctx.terminate("  ret void".to_string());
        }
        AsyncTerminator::Return(None) => {
            let promise = ctx.promise_ptr.clone().unwrap();
            if ret_ty.is_void() {
                ctx.promise_resolve(&promise, None);
            } else {
                let value = ctx.default_value(ret_ty);
                ctx.promise_resolve(&promise, Some(&value));
            }
            ctx.terminate("  ret void".to_string());
        }
        AsyncTerminator::Jump(target) => {
            ctx.terminate(format!("  br label %{}", cfg_block_label(*target)));
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
            emit_async_cfg_await(ctx, fields, cfg, awaits, await_term);
        }
        AsyncTerminator::Unreachable => {
            ctx.terminate("  unreachable".to_string());
        }
    }
}

fn emit_async_cfg_await(
    ctx: &mut FunctionCtx<'_>,
    fields: &HashMap<String, FrameField>,
    _cfg: &AsyncCfgFunction,
    awaits: &[AwaitPointInfo],
    await_term: &AwaitTerminator,
) {
    let await_info = awaits
        .iter()
        .find(|info| info.state == await_term.state)
        .unwrap_or_else(|| panic!("unknown await state {}", await_term.state));
    let child = emit_exp(ctx, &await_term.child);
    async_store_field(
        ctx,
        "%frame",
        fields.get(&await_info.child_field).unwrap(),
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
            let promise = ctx.promise_ptr.clone().unwrap();
            if value.ty.is_void() {
                ctx.promise_resolve(&promise, None);
            } else {
                ctx.promise_resolve(&promise, Some(&value));
            }
            ctx.terminate("  ret void".to_string());
            return;
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
}

fn await_resume_block(cfg: &AsyncCfgFunction, state: i32) -> Option<usize> {
    cfg.await_points()
        .into_iter()
        .find(|await_term| await_term.state == state)
        .map(|await_term| await_term.resume_block)
}

fn cfg_block_label(block: usize) -> String {
    format!("cfg.block.{}", block)
}

fn emit_async_main_driver(module: &ModuleCtx, _hidden: &FuncDef) -> String {
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

fn emit_global_decl(out: &mut String, module: &ModuleCtx, ty: &Type, decls: &[SingleDecl]) {
    for decl in decls {
        let name = var_decl_name(&decl.var);
        let value_ty = decl_type(ty, &decl.var, module);
        if module.constants.contains_key(&name) {
            continue;
        }
        let init = match (&value_ty, &decl.init) {
            (LlvmType::I32, Some(init)) => init_const(init, module).unwrap_or(0).to_string(),
            (LlvmType::I32, None) => "0".to_string(),
            (_, Some(init)) => const_initializer(init, &value_ty, module)
                .unwrap_or_else(|| zero_initializer(&value_ty)),
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
}

fn emit_stmts(ctx: &mut FunctionCtx<'_>, stmts: &[Stmt]) {
    ctx.push_scope();
    for stmt in stmts {
        if ctx.current_terminated {
            break;
        }
        emit_stmt(ctx, stmt);
    }
    ctx.pop_scope();
}

fn emit_stmt(ctx: &mut FunctionCtx<'_>, stmt: &Stmt) {
    match stmt {
        Stmt::Block(stmts) => emit_stmts(ctx, stmts),
        Stmt::Assign(lhs, rhs) => {
            let value = emit_exp(ctx, rhs);
            let lvalue = emit_lvalue(ctx, lhs);
            store_value_to_ptr(ctx, &value, &lvalue.ty, &lvalue.ptr);
        }
        Stmt::Decl(ty, decls) => {
            for decl in decls {
                let name = var_decl_name(&decl.var);
                let value_ty = decl_type(ty, &decl.var, ctx.module);
                if matches!(ty, Type::Const(_)) && matches!(value_ty, LlvmType::I32) {
                    if let Some(init) = &decl.init {
                        if let Some(value) = init_const(init, ctx.module) {
                            ctx.module.constants.get(&name).or(Some(&value));
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
                );
                if let Some(init) = &decl.init {
                    init_store(ctx, init, &value_ty, &ptr);
                }
            }
        }
        Stmt::Exp(exp) => {
            let _ = emit_exp(ctx, exp);
        }
        Stmt::PromiseWait(exp) => {
            let promise = emit_exp(ctx, exp);
            let _ = ctx.promise_wait(promise);
        }
        Stmt::Return(Some(exp)) => {
            let mut value = emit_exp(ctx, exp);
            if ctx.is_async {
                let promise = ctx.promise_ptr.clone().unwrap();
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
        }
        Stmt::Return(None) => {
            if ctx.is_async {
                let promise = ctx.promise_ptr.clone().unwrap();
                ctx.promise_resolve(&promise, None);
                ctx.terminate(format!("  ret ptr {}", promise));
            } else {
                ctx.terminate("  ret void".to_string());
            }
        }
        Stmt::If(cond, then_stmt) => {
            let then_label = ctx.label("if.then");
            let end_label = ctx.label("if.end");
            emit_cond_br(ctx, cond, &then_label, &end_label);
            ctx.emit_label(&then_label);
            emit_stmt(ctx, then_stmt);
            if !ctx.current_terminated {
                ctx.terminate(format!("  br label %{}", end_label));
            }
            ctx.emit_label(&end_label);
        }
        Stmt::IfElse(cond, then_stmt, else_stmt) => {
            let then_label = ctx.label("if.then");
            let else_label = ctx.label("if.else");
            let end_label = ctx.label("if.end");
            emit_cond_br(ctx, cond, &then_label, &else_label);
            ctx.emit_label(&then_label);
            emit_stmt(ctx, then_stmt);
            if !ctx.current_terminated {
                ctx.terminate(format!("  br label %{}", end_label));
            }
            ctx.emit_label(&else_label);
            emit_stmt(ctx, else_stmt);
            if !ctx.current_terminated {
                ctx.terminate(format!("  br label %{}", end_label));
            }
            ctx.emit_label(&end_label);
        }
        Stmt::While(cond, body) => {
            let cond_label = ctx.label("while.cond");
            let body_label = ctx.label("while.body");
            let end_label = ctx.label("while.end");
            ctx.terminate(format!("  br label %{}", cond_label));
            ctx.emit_label(&cond_label);
            emit_cond_br(ctx, cond, &body_label, &end_label);
            ctx.emit_label(&body_label);
            ctx.loop_stack.push(LoopLabels {
                break_label: end_label.clone(),
                continue_label: cond_label.clone(),
            });
            emit_stmt(ctx, body);
            ctx.loop_stack.pop();
            if !ctx.current_terminated {
                ctx.terminate(format!("  br label %{}", cond_label));
            }
            ctx.emit_label(&end_label);
        }
        Stmt::Break => {
            let labels = ctx
                .loop_stack
                .last()
                .unwrap_or_else(|| panic!("break used outside a loop"));
            ctx.terminate(format!("  br label %{}", labels.break_label));
        }
        Stmt::Continue => {
            let labels = ctx
                .loop_stack
                .last()
                .unwrap_or_else(|| panic!("continue used outside a loop"));
            ctx.terminate(format!("  br label %{}", labels.continue_label));
        }
        Stmt::Empty => {}
    }
}

fn emit_exp(ctx: &mut FunctionCtx<'_>, exp: &Exp) -> Value {
    match exp {
        Exp::Number(value) => Value {
            name: value.to_string(),
            ty: LlvmType::I32,
        },
        Exp::Ident(name) => {
            if let Some(value) = ctx.module.constants.get(name) {
                return Value {
                    name: value.to_string(),
                    ty: LlvmType::I32,
                };
            }
            if let Some(var) = ctx.lookup_var(name).cloned() {
                match var.ty {
                    LlvmType::Struct(_) | LlvmType::Array(_, _) => Value {
                        name: var.ptr,
                        ty: LlvmType::Ptr(Box::new(var.ty)),
                    },
                    _ => ctx.load(&var.ptr, &var.ty),
                }
            } else if let Some(global_ty) = ctx.module.globals.get(name).cloned() {
                match global_ty {
                    LlvmType::Struct(_) | LlvmType::Array(_, _) => Value {
                        name: format!("@{}", sanitize_ident(name)),
                        ty: LlvmType::Ptr(Box::new(global_ty)),
                    },
                    _ => ctx.load(&format!("@{}", sanitize_ident(name)), &global_ty),
                }
            } else {
                panic!("Unknown identifier {}", name);
            }
        }
        Exp::UnaryExp(UnaryOp::Addr, inner) => {
            let lvalue = emit_lvalue(ctx, inner);
            Value {
                name: lvalue.ptr,
                ty: LlvmType::Ptr(Box::new(lvalue.ty)),
            }
        }
        Exp::UnaryExp(UnaryOp::Deref, inner) => {
            let ptr = emit_exp(ctx, inner);
            match ptr.ty {
                LlvmType::Ptr(inner_ty) => ctx.load(&ptr.name, &inner_ty),
                other => panic!("Cannot dereference {:?}", other),
            }
        }
        Exp::UnaryExp(op, inner) => {
            let value = emit_exp(ctx, inner);
            match op {
                UnaryOp::Pos => value,
                UnaryOp::Neg => {
                    let out = ctx.tmp();
                    ctx.emit(format!("  {} = sub i32 0, {}", out, value.name));
                    Value {
                        name: out,
                        ty: LlvmType::I32,
                    }
                }
                UnaryOp::Not => {
                    let cmp = ctx.tmp();
                    let out = ctx.tmp();
                    ctx.emit(format!("  {} = icmp eq i32 {}, 0", cmp, value.name));
                    ctx.emit(format!("  {} = zext i1 {} to i32", out, cmp));
                    Value {
                        name: out,
                        ty: LlvmType::I32,
                    }
                }
                UnaryOp::Addr | UnaryOp::Deref => unreachable!(),
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
                value_ty.size(&ctx.module.layouts)
            ));
            Value {
                name: ptr,
                ty: LlvmType::Ptr(Box::new(value_ty)),
            }
        }
        Exp::Await(inner) => {
            let promise = emit_exp(ctx, inner);
            ctx.promise_wait(promise)
        }
        Exp::Sleep(duration) => {
            let value = emit_exp(ctx, duration);
            let promise = ctx.tmp();
            ctx.emit(format!(
                "  {} = call ptr @__sysy_sleep(i32 {})",
                promise, value.name
            ));
            Value {
                name: promise,
                ty: LlvmType::Promise(Box::new(LlvmType::Void)),
            }
        }
        Exp::PromiseWait(inner) => {
            let promise = emit_exp(ctx, inner);
            ctx.promise_wait(promise)
        }
        Exp::FuncCall(name, args) => {
            let values = args
                .iter()
                .map(|arg| emit_exp(ctx, arg))
                .collect::<Vec<_>>();
            call_function(ctx, name, &values)
        }
        Exp::ArrGet(_, _) | Exp::Field(_, _) | Exp::PtrField(_, _) => {
            let lvalue = emit_lvalue(ctx, exp);
            match lvalue.ty {
                LlvmType::Struct(_) | LlvmType::Array(_, _) => Value {
                    name: lvalue.ptr,
                    ty: LlvmType::Ptr(Box::new(lvalue.ty)),
                },
                _ => ctx.load(&lvalue.ptr, &lvalue.ty),
            }
        }
    }
}

fn emit_binary(ctx: &mut FunctionCtx<'_>, op: &BinaryOp, lhs: &Exp, rhs: &Exp) -> Value {
    if matches!(op, BinaryOp::And | BinaryOp::Or) {
        return emit_short_circuit(ctx, op, lhs, rhs);
    }

    let l = emit_exp(ctx, lhs);
    let r = emit_exp(ctx, rhs);
    let out = ctx.tmp();
    match op {
        BinaryOp::Add => ctx.emit(format!("  {} = add i32 {}, {}", out, l.name, r.name)),
        BinaryOp::Sub => ctx.emit(format!("  {} = sub i32 {}, {}", out, l.name, r.name)),
        BinaryOp::Mul => ctx.emit(format!("  {} = mul i32 {}, {}", out, l.name, r.name)),
        BinaryOp::Div => ctx.emit(format!("  {} = sdiv i32 {}, {}", out, l.name, r.name)),
        BinaryOp::Mod => ctx.emit(format!("  {} = srem i32 {}, {}", out, l.name, r.name)),
        BinaryOp::And | BinaryOp::Or => unreachable!(),
        cmp => {
            let pred = match cmp {
                BinaryOp::Lt => "slt",
                BinaryOp::Gt => "sgt",
                BinaryOp::Le => "sle",
                BinaryOp::Ge => "sge",
                BinaryOp::Eq => "eq",
                BinaryOp::Ne => "ne",
                _ => unreachable!(),
            };
            let cmp_tmp = ctx.tmp();
            ctx.emit(format!(
                "  {} = icmp {} i32 {}, {}",
                cmp_tmp, pred, l.name, r.name
            ));
            ctx.emit(format!("  {} = zext i1 {} to i32", out, cmp_tmp));
        }
    }
    Value {
        name: out,
        ty: LlvmType::I32,
    }
}

fn emit_short_circuit(ctx: &mut FunctionCtx<'_>, op: &BinaryOp, lhs: &Exp, rhs: &Exp) -> Value {
    let lhs_value = emit_exp(ctx, lhs);
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
        _ => unreachable!(),
    }

    ctx.emit_label(&rhs_label);
    let rhs_value = emit_exp(ctx, rhs);
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
        _ => unreachable!(),
    };
    ctx.emit(format!(
        "  {} = phi i1 [{}, %{}], [{}, %{}]",
        phi, short_value, lhs_label, rhs_bool, rhs_pred
    ));
    let out = ctx.tmp();
    ctx.emit(format!("  {} = zext i1 {} to i32", out, phi));
    Value {
        name: out,
        ty: LlvmType::I32,
    }
}

fn emit_lvalue(ctx: &mut FunctionCtx<'_>, exp: &Exp) -> LValue {
    match exp {
        Exp::Ident(name) => {
            if let Some(var) = ctx.lookup_var(name).cloned() {
                LValue {
                    ptr: var.ptr,
                    ty: var.ty,
                }
            } else if let Some(ty) = ctx.module.globals.get(name).cloned() {
                LValue {
                    ptr: format!("@{}", sanitize_ident(name)),
                    ty,
                }
            } else {
                panic!("Unknown lvalue {}", name);
            }
        }
        Exp::UnaryExp(UnaryOp::Deref, inner) => {
            let ptr = emit_exp(ctx, inner);
            match ptr.ty {
                LlvmType::Ptr(inner_ty) => LValue {
                    ptr: ptr.name,
                    ty: *inner_ty,
                },
                other => panic!("Cannot dereference lvalue {:?}", other),
            }
        }
        Exp::ArrGet(base, index) => {
            let base_lv = emit_lvalue(ctx, base);
            let idx = emit_exp(ctx, index);
            let elem_ty = match &base_lv.ty {
                LlvmType::Array(_, inner) => (**inner).clone(),
                LlvmType::Ptr(inner) => (**inner).clone(),
                other => panic!("Cannot index into {:?}", other),
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
                _ => unreachable!(),
            }
            LValue {
                ptr: gep,
                ty: elem_ty,
            }
        }
        Exp::Field(base, field_name) => {
            let base_lv = emit_lvalue(ctx, base);
            let struct_name = match &base_lv.ty {
                LlvmType::Struct(name) => name.clone(),
                other => panic!("Cannot access field on {:?}", other),
            };
            let field = ctx.module.field(&struct_name, field_name);
            let gep = ctx.tmp();
            ctx.emit(format!(
                "  {} = getelementptr inbounds {}, ptr {}, i32 0, i32 {}",
                gep,
                LlvmType::Struct(struct_name).llvm(),
                base_lv.ptr,
                field.index
            ));
            LValue {
                ptr: gep,
                ty: field.ty.clone(),
            }
        }
        Exp::PtrField(base, field_name) => {
            let base_value = emit_exp(ctx, base);
            let struct_name = match &base_value.ty {
                LlvmType::Ptr(inner) => match &**inner {
                    LlvmType::Struct(name) => name.clone(),
                    other => panic!("Cannot access pointer field through {:?}", other),
                },
                other => panic!("Cannot access pointer field through {:?}", other),
            };
            let field = ctx.module.field(&struct_name, field_name);
            let gep = ctx.tmp();
            ctx.emit(format!(
                "  {} = getelementptr inbounds {}, ptr {}, i32 0, i32 {}",
                gep,
                LlvmType::Struct(struct_name).llvm(),
                base_value.name,
                field.index
            ));
            LValue {
                ptr: gep,
                ty: field.ty.clone(),
            }
        }
        other => panic!("Expression {:?} is not assignable", other),
    }
}

fn emit_cond_br(ctx: &mut FunctionCtx<'_>, cond: &Exp, then_label: &str, else_label: &str) {
    let value = emit_exp(ctx, cond);
    let cmp = ctx.tmp();
    ctx.emit(format!("  {} = icmp ne i32 {}, 0", cmp, value.name));
    ctx.terminate(format!(
        "  br i1 {}, label %{}, label %{}",
        cmp, then_label, else_label
    ));
}

fn call_function(ctx: &mut FunctionCtx<'_>, name: &str, args: &[Value]) -> Value {
    let sig = ctx
        .module
        .funcs
        .get(name)
        .unwrap_or_else(|| panic!("Unknown function {}", name))
        .clone();
    let ret_ty = if sig.is_async {
        LlvmType::Promise(Box::new(sig.ret.clone()))
    } else {
        sig.ret.clone()
    };
    let adapted_args = args
        .iter()
        .enumerate()
        .map(|(idx, arg)| {
            let expected = sig.params.get(idx).unwrap_or(&arg.ty);
            adapt_call_arg(ctx, arg, expected)
        })
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
        Value {
            name: "0".to_string(),
            ty: LlvmType::Void,
        }
    } else {
        let out = ctx.tmp();
        ctx.emit(format!(
            "  {} = call {} @{}({})",
            out,
            ret_ty.llvm(),
            sanitize_ident(name),
            args_text
        ));
        Value {
            name: out,
            ty: ret_ty,
        }
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

fn init_store(ctx: &mut FunctionCtx<'_>, init: &InitVal, ty: &LlvmType, ptr: &str) {
    match init {
        InitVal::Exp(exp) => {
            let value = emit_exp(ctx, exp);
            store_value_to_ptr(ctx, &value, ty, ptr);
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
                    init_store(ctx, item, &field.ty, &field_ptr);
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
                    let value = emit_exp(ctx, &exp);
                    store_value_to_ptr(ctx, &value, inner_scalar_type(ty), &scalar_ptr);
                }
            }
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
            match item {
                InitVal::Exp(exp) => {
                    out.push((base + cursor, exp.clone()));
                    cursor += 1;
                }
                InitVal::Arr(nested) => {
                    flatten_runtime_items(nested, inner, base + cursor, out);
                    cursor += inner_count;
                }
            }
        }
    } else if let Some(InitVal::Exp(exp)) = items.first() {
        out.push((base, exp.clone()));
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

fn store_value_to_ptr(ctx: &mut FunctionCtx<'_>, value: &Value, dest_ty: &LlvmType, ptr: &str) {
    match (dest_ty, &value.ty) {
        (LlvmType::Struct(_), LlvmType::Ptr(inner)) if **inner == *dest_ty => {
            let loaded = ctx.load(&value.name, dest_ty);
            ctx.store(&loaded, ptr);
        }
        _ => ctx.store(value, ptr),
    }
}

fn type_to_llvm(ty: &Type) -> LlvmType {
    match ty {
        Type::BType(btype) | Type::Const(btype) => LlvmType::from_btype(btype),
    }
}

fn decl_type(ty: &Type, var: &VarDecl, module: &ModuleCtx) -> LlvmType {
    let base = type_to_llvm(ty);
    apply_var_dims(base, var, module)
}

fn apply_var_dims(base: LlvmType, var: &VarDecl, module: &ModuleCtx) -> LlvmType {
    let mut dims = Vec::new();
    collect_var_dims(var, module, &mut dims);
    let mut ty = base;
    for dim in dims.into_iter().rev() {
        ty = LlvmType::Array(dim, Box::new(ty));
    }
    ty
}

fn collect_var_dims(var: &VarDecl, module: &ModuleCtx, dims: &mut Vec<usize>) {
    match var {
        VarDecl::Ident(_) => {}
        VarDecl::Array(inner, len) => {
            collect_var_dims(inner, module, dims);
            let len = eval_const_exp_with(len, &|name| module.constants.get(name).copied())
                .expect("Array length must be a constant expression");
            dims.push(len as usize);
        }
    }
}

fn param_resolved_btype(param: &FuncParam, module: &ModuleCtx) -> BType {
    if let Some(dims) = &param.array_dims {
        let dims = dims
            .iter()
            .map(|dim| eval_param_dim(dim, &|name| module.constants.get(name).copied()))
            .collect::<Vec<_>>();
        func_param_btype(param.btype.clone(), dims)
    } else {
        param.btype.clone()
    }
}

fn var_decl_name(var: &VarDecl) -> String {
    match var {
        VarDecl::Ident(name) => name.clone(),
        VarDecl::Array(inner, _) => var_decl_name(inner),
    }
}

fn init_const(init: &InitVal, module: &ModuleCtx) -> Option<i32> {
    match init {
        InitVal::Exp(exp) => eval_const_exp_with(exp, &|name| module.constants.get(name).copied()),
        InitVal::Arr(_) => None,
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
            match item {
                InitVal::Exp(exp) => {
                    flat[base + cursor] =
                        eval_const_exp_with(exp, &|name| module.constants.get(name).copied())?
                            .to_string();
                    cursor += 1;
                }
                InitVal::Arr(nested) => {
                    flatten_const_items(nested, inner, base + cursor, flat, module)?;
                    cursor += inner_count;
                }
            }
        }
        Some(())
    } else if let Some(InitVal::Exp(exp)) = items.first() {
        flat[base] =
            eval_const_exp_with(exp, &|name| module.constants.get(name).copied())?.to_string();
        Some(())
    } else {
        Some(())
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

fn stable_structs(layouts: &HashMap<String, StructLayout>) -> Vec<(&String, &StructLayout)> {
    let mut items = layouts.iter().collect::<Vec<_>>();
    items.sort_by(|lhs, rhs| lhs.0.cmp(rhs.0));
    items
}

fn sanitize_ident(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}
