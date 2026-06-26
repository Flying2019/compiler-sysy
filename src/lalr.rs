use std::collections::{HashMap, HashSet};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub span: Option<Span>,
    pub message: String,
}

impl Diagnostic {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            span: None,
            message: message.into(),
        }
    }

    pub fn at(span: Span, message: impl Into<String>) -> Self {
        Self {
            span: Some(span),
            message: message.into(),
        }
    }

    pub fn with_fallback_span(mut self, span: Span) -> Self {
        if self.span.is_none() {
            self.span = Some(span);
        }
        self
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(f)
    }
}

impl std::error::Error for Diagnostic {}

impl From<String> for Diagnostic {
    fn from(message: String) -> Self {
        Self::new(message)
    }
}

impl From<&str> for Diagnostic {
    fn from(message: &str) -> Self {
        Self::new(message)
    }
}

fn diagnostic_at(span: Option<Span>, message: impl Into<String>) -> Diagnostic {
    match span {
        Some(span) => Diagnostic::at(span, message),
        None => Diagnostic::new(message),
    }
}

#[derive(Debug, Clone)]
pub struct CompUnit {
    pub global_defs: Vec<GlobalDef>,
}

#[derive(Debug, Clone)]
pub struct AsyncProgramAnalysis {
    pub functions: Vec<AsyncFunctionAnalysis>,
}

#[derive(Debug, Clone)]
pub struct AsyncFunctionAnalysis {
    pub name: String,
    pub await_points: Vec<AwaitPoint>,
    pub frame_local_candidates: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AwaitPoint {
    pub continuation_id: usize,
    pub in_loop: bool,
}

#[derive(Debug, Clone)]
struct FunctionSig {
    is_async: bool,
    ret: BType,
    param_count: usize,
    params: Vec<BType>,
}

#[derive(Debug, Clone)]
struct StructSig {
    fields: HashMap<String, BType>,
    field_order: Vec<StructFieldSig>,
}

#[derive(Debug, Clone)]
struct StructFieldSig {
    name: String,
    ty: BType,
    span: Option<Span>,
}

#[derive(Debug, Clone)]
struct SemanticProgram {
    funcs: HashMap<String, FunctionSig>,
    structs: HashMap<String, StructSig>,
    globals: HashMap<String, BType>,
    constants: HashMap<String, i32>,
}

#[derive(Debug, Clone)]
pub enum GlobalDef {
    FuncDef(FuncDef),
    GlobalDecl(Type, Vec<SingleDecl>), // @global = decl type, init
    StructDef(StructDef),
}

#[derive(Debug, Clone)]
pub struct StructDef {
    pub span: Option<Span>,
    pub name: String,
    pub fields: Vec<StructField>,
}

#[derive(Debug, Clone)]
pub struct StructField {
    pub span: Option<Span>,
    pub ty: Type,
    pub decls: Vec<SingleDecl>,
}

#[derive(Debug, Clone)]
pub struct FuncDef {
    pub span: Option<Span>,
    pub is_async: bool,
    pub func_type: Type,
    pub ident: String,
    pub func_params: Vec<FuncParam>,
    pub block: Vec<Stmt>,
}

#[derive(Debug, Clone)]
pub struct FuncParam {
    pub btype: BType,
    pub name: String,
    pub array_dims: Option<Vec<Exp>>,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Spanned(Box<Stmt>, Span),
    Block(Vec<Stmt>),
    Assign(Exp, Exp),
    Decl(Type, Vec<SingleDecl>),
    Exp(Exp),
    If(Exp, Box<Stmt>),
    IfElse(Exp, Box<Stmt>, Box<Stmt>),
    While(Exp, Box<Stmt>),
    PromiseWait(Exp),
    Continue,
    Break,
    Return(Option<Exp>),
    Empty,
}

pub fn spanned_stmt(stmt: Stmt, start: usize, end: usize) -> Stmt {
    Stmt::Spanned(Box::new(stmt), Span { start, end })
}

#[derive(Debug, Clone)]
pub enum VarDecl {
    Ident(String),
    Array(Box<VarDecl>, Exp),
}

#[derive(Debug, Clone)]
pub struct SingleDecl {
    pub span: Option<Span>,
    pub var: VarDecl,
    pub init: Option<InitVal>,
}

#[derive(Debug, Clone)]
pub enum InitVal {
    Spanned(Box<InitVal>, Span),
    Exp(Exp),
    Arr(Vec<InitVal>),
}

pub fn spanned_init(init: InitVal, start: usize, end: usize) -> InitVal {
    InitVal::Spanned(Box::new(init), Span { start, end })
}

#[derive(Debug, Clone)]
pub enum BType {
    I32,
    Void,
    Struct(String),
    Promise(Box<BType>),
    Ptr(Box<BType>),
    Array(usize, Box<BType>),
}

pub fn func_param_btype(base: BType, dims: Vec<usize>) -> BType {
    let mut ty = base;
    for dim in dims.into_iter().rev() {
        ty = BType::Array(dim, Box::new(ty));
    }
    BType::Ptr(Box::new(ty))
}

#[derive(Debug, Clone)]
pub enum Type {
    BType(BType),
    Const(BType),
}

pub fn param_base_btype(ty: Type) -> BType {
    match ty {
        Type::BType(btype) | Type::Const(btype) => btype,
    }
}

pub fn eval_const_exp_with<F>(exp: &Exp, lookup: &F) -> Option<i32>
where
    F: Fn(&str) -> Option<i32>,
{
    match exp {
        Exp::Spanned(exp, _) => eval_const_exp_with(exp, lookup),
        Exp::Number(num) => Some(*num),
        Exp::UnaryExp(op, inner) => {
            let value = eval_const_exp_with(inner, lookup)?;
            Some(match op {
                UnaryOp::Pos => value,
                UnaryOp::Neg => -value,
                UnaryOp::Not => {
                    if value == 0 {
                        1
                    } else {
                        0
                    }
                }
                UnaryOp::Addr | UnaryOp::Deref => return None,
            })
        }
        Exp::BinaryExp(op, lhs, rhs) => {
            let lhs = eval_const_exp_with(lhs, lookup)?;
            let rhs = eval_const_exp_with(rhs, lookup)?;
            Some(match op {
                BinaryOp::Add => lhs + rhs,
                BinaryOp::Sub => lhs - rhs,
                BinaryOp::Mul => lhs * rhs,
                BinaryOp::Div => {
                    if rhs == 0 {
                        return None;
                    }
                    lhs / rhs
                }
                BinaryOp::Mod => {
                    if rhs == 0 {
                        return None;
                    }
                    lhs % rhs
                }
                BinaryOp::Lt => (lhs < rhs) as i32,
                BinaryOp::Gt => (lhs > rhs) as i32,
                BinaryOp::Le => (lhs <= rhs) as i32,
                BinaryOp::Ge => (lhs >= rhs) as i32,
                BinaryOp::Eq => (lhs == rhs) as i32,
                BinaryOp::Ne => (lhs != rhs) as i32,
                BinaryOp::And => ((lhs != 0) && (rhs != 0)) as i32,
                BinaryOp::Or => ((lhs != 0) || (rhs != 0)) as i32,
            })
        }
        Exp::Ident(name) => lookup(name),
        Exp::New(_)
        | Exp::Await(_)
        | Exp::Sleep(_)
        | Exp::PromiseWait(_)
        | Exp::FuncCall(_, _)
        | Exp::ArrGet(_, _)
        | Exp::Field(_, _)
        | Exp::PtrField(_, _) => None,
    }
}

pub fn eval_param_dim<F>(exp: &Exp, lookup: &F) -> Result<usize, String>
where
    F: Fn(&str) -> Option<i32>,
{
    let value = eval_const_exp_with(exp, lookup)
        .ok_or_else(|| "Array parameter dimensions must be constant expressions".to_string())?;
    if value < 0 {
        return Err("Array parameter dimensions must be non-negative".to_string());
    }
    Ok(value as usize)
}

#[derive(Debug, Clone)]
pub enum Exp {
    Spanned(Box<Exp>, Span),
    Number(i32),
    UnaryExp(UnaryOp, Box<Exp>),
    BinaryExp(BinaryOp, Box<Exp>, Box<Exp>),
    New(BType),
    Await(Box<Exp>),
    Sleep(Box<Exp>),
    PromiseWait(Box<Exp>),
    FuncCall(String, Vec<Exp>),
    ArrGet(Box<Exp>, Box<Exp>),
    Field(Box<Exp>, String),
    PtrField(Box<Exp>, String),
    Ident(String),
}

pub fn spanned_exp(exp: Exp, start: usize, end: usize) -> Exp {
    Exp::Spanned(Box::new(exp), Span { start, end })
}

impl CompUnit {
    pub fn contains_async_syntax(&self) -> bool {
        self.global_defs
            .iter()
            .any(GlobalDef::contains_async_syntax)
    }

    pub fn validate_async_syntax(&self) -> Result<(), Diagnostic> {
        self.validate_semantics()
    }

    pub fn validate_semantics(&self) -> Result<(), Diagnostic> {
        let program = self.semantic_program()?;
        for glob_def in &self.global_defs {
            glob_def.validate_semantics(&program)?;
        }
        Ok(())
    }

    pub fn analyze_async(&self) -> AsyncProgramAnalysis {
        let functions = self
            .global_defs
            .iter()
            .filter_map(|glob_def| match glob_def {
                GlobalDef::FuncDef(func) if func.is_async => Some(func.analyze_async()),
                _ => None,
            })
            .collect();
        AsyncProgramAnalysis { functions }
    }

    fn semantic_program(&self) -> Result<SemanticProgram, Diagnostic> {
        let mut funcs = builtin_function_sigs();
        let mut structs = HashMap::new();
        let mut globals = HashMap::new();
        let constants = collect_global_constants(&self.global_defs);
        let mut struct_names = HashSet::new();
        for glob_def in &self.global_defs {
            if let GlobalDef::StructDef(def) = glob_def {
                if !struct_names.insert(def.name.clone()) {
                    return Err(diagnostic_at(
                        def.span,
                        format!("Duplicate struct definition: {}", def.name),
                    ));
                }
            }
        }
        for glob_def in &self.global_defs {
            if let GlobalDef::StructDef(def) = glob_def {
                let mut fields = HashMap::new();
                let mut field_order = Vec::new();
                for field in &def.fields {
                    for decl in &field.decls {
                        let span = decl.span.or(field.span);
                        let field_ty = decl_btype(&field.ty, &decl.var, &constants)
                            .map_err(|err| diagnostic_at(span, err))?;
                        validate_object_type(&field_ty, &struct_names, "Struct field")
                            .map_err(|err| diagnostic_at(span, err))?;
                        let field_name = var_decl_name(&decl.var);
                        if fields
                            .insert(field_name.clone(), field_ty.clone())
                            .is_some()
                        {
                            return Err(diagnostic_at(
                                span,
                                format!("Duplicate field {} in struct {}", field_name, def.name),
                            ));
                        }
                        field_order.push(StructFieldSig {
                            name: field_name,
                            ty: field_ty,
                            span,
                        });
                    }
                }
                structs.insert(
                    def.name.clone(),
                    StructSig {
                        fields,
                        field_order,
                    },
                );
            }
        }
        for glob_def in &self.global_defs {
            if let GlobalDef::GlobalDecl(ty, decls) = glob_def {
                for decl in decls {
                    let name = var_decl_name(&decl.var);
                    let decl_ty = decl_btype(ty, &decl.var, &constants)
                        .map_err(|err| diagnostic_at(decl.span, err))?;
                    validate_object_type(&decl_ty, &struct_names, "Global variable")
                        .map_err(|err| diagnostic_at(decl.span, err))?;
                    if funcs.contains_key(&name) || globals.insert(name.clone(), decl_ty).is_some()
                    {
                        return Err(diagnostic_at(
                            decl.span,
                            format!("Duplicate global definition: {}", name),
                        ));
                    }
                }
            }
        }
        for glob_def in &self.global_defs {
            if let GlobalDef::FuncDef(func) = glob_def {
                validate_return_type(type_base_btype(&func.func_type), &struct_names)
                    .map_err(|err| diagnostic_at(func.span, err))?;
                let mut params = Vec::new();
                let mut param_names = HashSet::new();
                for param in &func.func_params {
                    if !param_names.insert(param.name.clone()) {
                        return Err(diagnostic_at(
                            func.span,
                            format!(
                                "Duplicate parameter {} in function {}",
                                param.name, func.ident
                            ),
                        ));
                    }
                    let param_ty = func_param_semantic_btype(param, &constants)
                        .map_err(|err| diagnostic_at(func.span, err))?;
                    validate_object_type(&param_ty, &struct_names, "Function parameter")
                        .map_err(|err| diagnostic_at(func.span, err))?;
                    params.push(param_ty);
                }
                if globals.contains_key(&func.ident) {
                    return Err(diagnostic_at(
                        func.span,
                        format!("Duplicate global definition: {}", func.ident),
                    ));
                }
                if funcs
                    .insert(
                        func.ident.clone(),
                        FunctionSig {
                            is_async: func.is_async,
                            ret: type_base_btype(&func.func_type).clone(),
                            param_count: func.func_params.len(),
                            params,
                        },
                    )
                    .is_some()
                {
                    return Err(diagnostic_at(
                        func.span,
                        format!("Duplicate function definition: {}", func.ident),
                    ));
                }
            }
        }
        validate_struct_value_cycles(&structs)?;
        Ok(SemanticProgram {
            funcs,
            structs,
            globals,
            constants,
        })
    }
}

impl GlobalDef {
    fn contains_async_syntax(&self) -> bool {
        match self {
            GlobalDef::FuncDef(func) => func.contains_async_syntax(),
            GlobalDef::GlobalDecl(ty, decls) => {
                type_contains_promise(ty)
                    || decls.iter().any(|decl| {
                        decl.init
                            .as_ref()
                            .is_some_and(InitVal::contains_async_syntax)
                    })
            }
            GlobalDef::StructDef(def) => def
                .fields
                .iter()
                .any(|field| type_contains_promise(&field.ty)),
        }
    }

    fn validate_semantics(&self, program: &SemanticProgram) -> Result<(), Diagnostic> {
        match self {
            GlobalDef::FuncDef(func) => func.validate_semantics(program),
            GlobalDef::GlobalDecl(ty, decls) => {
                let mut ctx = SemanticCtx::new(program, false, &BType::Void);
                for decl in decls {
                    let decl_ty = decl_btype(ty, &decl.var, &program.constants)?;
                    if let Some(init) = &decl.init {
                        validate_initializer(&mut ctx, init, &decl_ty, 0)?;
                    }
                }
                Ok(())
            }
            GlobalDef::StructDef(_) => Ok(()),
        }
    }
}

impl FuncDef {
    fn contains_async_syntax(&self) -> bool {
        self.is_async
            || type_contains_promise(&self.func_type)
            || self
                .func_params
                .iter()
                .any(|param| btype_contains_promise(&param.btype))
            || self.block.iter().any(Stmt::contains_async_syntax)
    }

    pub fn analyze_async(&self) -> AsyncFunctionAnalysis {
        let mut analyzer = AsyncFunctionAnalyzer::new();
        analyzer.visit_stmts(&self.block, false);
        let mut frame_local_candidates = analyzer
            .frame_local_candidates
            .into_iter()
            .collect::<Vec<_>>();
        frame_local_candidates.sort();
        AsyncFunctionAnalysis {
            name: self.ident.clone(),
            await_points: analyzer.await_points,
            frame_local_candidates,
        }
    }

    fn validate_semantics(&self, program: &SemanticProgram) -> Result<(), Diagnostic> {
        let ret_ty = type_base_btype(&self.func_type);
        let mut ctx = SemanticCtx::new(program, self.is_async, ret_ty);
        ctx.push_scope();
        for param in &self.func_params {
            let param_ty = func_param_semantic_btype(param, &program.constants)
                .map_err(|err| diagnostic_at(self.span, err))?;
            ctx.define_var(param.name.clone(), param_ty)
                .map_err(|err| diagnostic_at(self.span, err))?;
        }
        for stmt in &self.block {
            stmt.validate_semantics(&mut ctx, 0)?;
        }
        ctx.finish_function()?;
        if requires_guaranteed_return(ret_ty) && !block_guarantees_return(&self.block) {
            let message = format!(
                "Function returning {} may fall through without return",
                btype_name(ret_ty)
            );
            return Err(self
                .span
                .map(|span| Diagnostic::at(span, message.clone()))
                .unwrap_or_else(|| Diagnostic::new(message)));
        }
        ctx.pop_scope();
        Ok(())
    }
}

impl Stmt {
    fn contains_async_syntax(&self) -> bool {
        match self {
            Stmt::Spanned(stmt, _) => stmt.contains_async_syntax(),
            Stmt::Block(stmts) => stmts.iter().any(Stmt::contains_async_syntax),
            Stmt::Assign(lhs, rhs) => lhs.contains_async_syntax() || rhs.contains_async_syntax(),
            Stmt::Decl(ty, decls) => {
                type_contains_promise(ty)
                    || decls.iter().any(|decl| {
                        decl.init
                            .as_ref()
                            .is_some_and(InitVal::contains_async_syntax)
                    })
            }
            Stmt::Exp(exp) | Stmt::Return(Some(exp)) | Stmt::PromiseWait(exp) => {
                exp.contains_async_syntax()
            }
            Stmt::If(cond, then_stmt) => {
                cond.contains_async_syntax() || then_stmt.contains_async_syntax()
            }
            Stmt::IfElse(cond, then_stmt, else_stmt) => {
                cond.contains_async_syntax()
                    || then_stmt.contains_async_syntax()
                    || else_stmt.contains_async_syntax()
            }
            Stmt::While(cond, body) => cond.contains_async_syntax() || body.contains_async_syntax(),
            Stmt::Continue | Stmt::Break | Stmt::Return(None) | Stmt::Empty => false,
        }
    }

    fn validate_semantics(
        &self,
        ctx: &mut SemanticCtx<'_>,
        loop_depth: usize,
    ) -> Result<(), Diagnostic> {
        match self {
            Stmt::Spanned(stmt, span) => stmt
                .validate_semantics(ctx, loop_depth)
                .map_err(|err| err.with_fallback_span(*span)),
            Stmt::Block(stmts) => {
                ctx.push_scope();
                for stmt in stmts {
                    stmt.validate_semantics(ctx, loop_depth)?;
                }
                ctx.pop_scope();
                Ok(())
            }
            Stmt::Assign(lhs, rhs) => {
                reject_unsupported_await_position(lhs, "assignment target")?;
                let lhs_kind = lhs.validate_semantics(ctx, loop_depth)?;
                let rhs_kind = rhs.validate_semantics(ctx, loop_depth)?;
                validate_assignment_kind(&lhs_kind, &rhs_kind).map_err(Diagnostic::from)?;
                if lhs_kind.is_promise() && rhs_kind.is_promise() {
                    ctx.consume_promise_exp(rhs);
                }
                Ok(())
            }
            Stmt::Decl(ty, decls) => {
                for decl in decls {
                    let constants = ctx.visible_constants();
                    let decl_ty = decl_btype(ty, &decl.var, &constants)
                        .map_err(|err| diagnostic_at(decl.span, err))?;
                    validate_object_type_in_program(&decl_ty, ctx.program, "Local variable")
                        .map_err(|err| diagnostic_at(decl.span, err))?;
                    if let Some(init) = &decl.init {
                        validate_initializer(ctx, init, &decl_ty, loop_depth)?;
                    }
                    ctx.define_const_from_decl(ty, decl);
                    ctx.define_var(var_decl_name(&decl.var), decl_ty)
                        .map_err(|err| diagnostic_at(decl.span, err))?;
                }
                Ok(())
            }
            Stmt::Exp(exp) => {
                let kind = exp.validate_semantics(ctx, loop_depth)?;
                if kind.is_promise() {
                    return Err(Diagnostic::new(
                        "Promise expression must be awaited, stored, or registered",
                    ));
                }
                Ok(())
            }
            Stmt::PromiseWait(exp) => {
                reject_unsupported_await_position(exp, "Promise.wait receiver")?;
                let kind = exp.validate_semantics(ctx, loop_depth)?;
                if !kind.is_promise() {
                    return Err(Diagnostic::new("Promise.wait() expects a Promise value"));
                }
                ctx.wait_promise_exp(exp);
                Ok(())
            }
            Stmt::Return(Some(exp)) => {
                let kind = exp.validate_semantics(ctx, loop_depth)?;
                if let AsyncExprKind::Promise(actual) = kind {
                    ctx.consume_promise_exp(exp);
                    ensure_promise_result_supported(&actual)?;
                    if let BType::Promise(expected) = ctx.ret_ty {
                        if btype_same(expected, &actual) {
                            return Ok(());
                        }
                        return Err(Diagnostic::new(format!(
                            "Return type mismatch: expected Promise<{}>, got Promise<{}>",
                            btype_name(expected),
                            btype_name(&actual)
                        )));
                    }
                    return Err(Diagnostic::new(
                        "Promise value cannot be returned without await",
                    ));
                }
                let actual_ty = kind.plain_ty().unwrap_or(&BType::Void);
                if matches!(actual_ty, BType::Array(_, _)) {
                    return Err(Diagnostic::new("Array value cannot be returned"));
                }
                if matches!(ctx.ret_ty, BType::Void) {
                    if !matches!(actual_ty, BType::Void) {
                        return Err(Diagnostic::new("Void function cannot return a value"));
                    }
                } else if !btype_same(ctx.ret_ty, actual_ty) {
                    return Err(Diagnostic::new(format!(
                        "Return type mismatch: expected {}, got {}",
                        btype_name(ctx.ret_ty),
                        btype_name(actual_ty)
                    )));
                }
                Ok(())
            }
            Stmt::If(cond, then_stmt) => {
                if cond.validate_semantics(ctx, loop_depth)?.is_promise() {
                    return Err(Diagnostic::new(
                        "Promise value cannot be used as an if condition",
                    ));
                }
                then_stmt.validate_semantics(ctx, loop_depth)
            }
            Stmt::IfElse(cond, then_stmt, else_stmt) => {
                if cond.validate_semantics(ctx, loop_depth)?.is_promise() {
                    return Err(Diagnostic::new(
                        "Promise value cannot be used as an if condition",
                    ));
                }
                then_stmt.validate_semantics(ctx, loop_depth)?;
                else_stmt.validate_semantics(ctx, loop_depth)
            }
            Stmt::While(cond, body) => {
                if cond.validate_semantics(ctx, loop_depth)?.is_promise() {
                    return Err(Diagnostic::new(
                        "Promise value cannot be used as a while condition",
                    ));
                }
                body.validate_semantics(ctx, loop_depth + 1)
            }
            Stmt::Break | Stmt::Continue if loop_depth == 0 => {
                Err(Diagnostic::new("break/continue used outside a loop"))
            }
            Stmt::Return(None) if !matches!(ctx.ret_ty, BType::Void) => {
                Err(Diagnostic::new(format!(
                    "Function returning {} cannot use empty return",
                    btype_name(ctx.ret_ty)
                )))
            }
            Stmt::Break | Stmt::Continue | Stmt::Return(None) | Stmt::Empty => Ok(()),
        }
    }
}

impl InitVal {
    fn contains_async_syntax(&self) -> bool {
        match self {
            InitVal::Spanned(init, _) => init.contains_async_syntax(),
            InitVal::Exp(exp) => exp.contains_async_syntax(),
            InitVal::Arr(items) => items.iter().any(InitVal::contains_async_syntax),
        }
    }
}

impl Exp {
    fn contains_async_syntax(&self) -> bool {
        match self {
            Exp::Spanned(exp, _) => exp.contains_async_syntax(),
            Exp::Await(_) | Exp::Sleep(_) => true,
            Exp::UnaryExp(_, exp) => exp.contains_async_syntax(),
            Exp::BinaryExp(_, lhs, rhs) | Exp::ArrGet(lhs, rhs) => {
                lhs.contains_async_syntax() || rhs.contains_async_syntax()
            }
            Exp::FuncCall(_, args) => args.iter().any(Exp::contains_async_syntax),
            Exp::Field(base, _) | Exp::PtrField(base, _) | Exp::PromiseWait(base) => {
                base.contains_async_syntax()
            }
            Exp::New(ty) => btype_contains_promise(ty),
            Exp::Number(_) | Exp::Ident(_) => false,
        }
    }

    fn validate_semantics(
        &self,
        ctx: &mut SemanticCtx<'_>,
        loop_depth: usize,
    ) -> Result<AsyncExprKind, Diagnostic> {
        match self {
            Exp::Spanned(exp, span) => exp
                .validate_semantics(ctx, loop_depth)
                .map_err(|err| err.with_fallback_span(*span)),
            Exp::Await(inner) => {
                if !ctx.in_async_fn {
                    return Err(Diagnostic::new(
                        "await is only allowed inside async functions",
                    ));
                }
                let kind = inner.validate_semantics(ctx, loop_depth)?;
                let AsyncExprKind::Promise(inner_ty) = kind else {
                    return Err(Diagnostic::new("await expects a Promise value"));
                };
                ensure_promise_result_supported(&inner_ty)?;
                ctx.await_promise_exp(inner);
                Ok(AsyncExprKind::Plain(inner_ty))
            }
            Exp::FuncCall(name, args) => {
                let sig = ctx
                    .program
                    .funcs
                    .get(name)
                    .ok_or_else(|| format!("Unknown function {}", name))?;
                if args.len() != sig.param_count {
                    return Err(Diagnostic::new(format!(
                        "Function {} expects {} argument(s), got {}",
                        name,
                        sig.param_count,
                        args.len()
                    )));
                }
                for (idx, arg) in args.iter().enumerate() {
                    let kind = arg.validate_semantics(ctx, loop_depth)?;
                    let expected = sig.params.get(idx).ok_or_else(|| {
                        format!(
                            "Function {} expects {} argument(s), got {}",
                            name,
                            sig.param_count,
                            args.len()
                        )
                    })?;
                    validate_call_arg_kind(name, idx, expected, &kind)?;
                    if matches!(expected, BType::Promise(_)) && kind.is_promise() {
                        ctx.consume_promise_exp(arg);
                    }
                }
                if sig.is_async {
                    ensure_promise_result_supported(&sig.ret)?;
                    Ok(AsyncExprKind::Promise(sig.ret.clone()))
                } else {
                    Ok(kind_from_btype(sig.ret.clone()))
                }
            }
            Exp::Sleep(duration) => {
                let duration_kind = duration.validate_semantics(ctx, loop_depth)?;
                if duration_kind.is_promise() {
                    return Err(Diagnostic::new("sleep duration cannot be a Promise value"));
                }
                let duration_ty = duration_kind.plain_ty().cloned().unwrap_or(BType::Void);
                expect_i32(&duration_ty, "sleep duration")?;
                Ok(AsyncExprKind::Promise(BType::Void))
            }
            Exp::PromiseWait(inner) | Exp::Field(inner, _) | Exp::PtrField(inner, _) => {
                if matches!(self, Exp::PromiseWait(_)) {
                    reject_unsupported_await_position(inner, "Promise.wait receiver")?;
                }
                let inner_kind = inner.validate_semantics(ctx, loop_depth)?;
                match self {
                    Exp::Field(_, field) => {
                        if inner_kind.is_promise() {
                            return Err(Diagnostic::new(
                                "Promise value cannot be used for field access without await",
                            ));
                        }
                        Ok(field_access_type(
                            ctx.program,
                            inner_kind.plain_ty(),
                            field,
                        )?)
                    }
                    Exp::PtrField(_, field) => {
                        if inner_kind.is_promise() {
                            return Err(Diagnostic::new(
                                "Promise value cannot be used for field access without await",
                            ));
                        }
                        Ok(ptr_field_access_type(
                            ctx.program,
                            inner_kind.plain_ty(),
                            field,
                        )?)
                    }
                    Exp::PromiseWait(_) => match inner_kind {
                        AsyncExprKind::Promise(inner_ty) => {
                            ensure_promise_result_supported(&inner_ty)?;
                            ctx.wait_promise_exp(inner);
                            Ok(AsyncExprKind::Plain(inner_ty))
                        }
                        AsyncExprKind::Plain(_) => {
                            Err(Diagnostic::new("Promise.wait() expects a Promise value"))
                        }
                    },
                    _ => Err(Diagnostic::new(
                        "internal semantic error: unexpected field or wait expression",
                    )),
                }
            }
            Exp::UnaryExp(op, inner) => {
                let inner_kind = inner.validate_semantics(ctx, loop_depth)?;
                if inner_kind.is_promise() {
                    return Err(Diagnostic::new(
                        "Promise value cannot be used in unary expression without await",
                    ));
                }
                validate_unary_type(op, inner_kind.plain_ty())?;
                match op {
                    UnaryOp::Addr => {
                        validate_address_operand(inner)?;
                        Ok(AsyncExprKind::Plain(BType::Ptr(Box::new(
                            inner_kind.plain_ty().cloned().unwrap_or(BType::I32),
                        ))))
                    }
                    UnaryOp::Deref => match inner_kind.plain_ty() {
                        Some(BType::Ptr(inner)) => Ok(AsyncExprKind::Plain((**inner).clone())),
                        _ => Err(Diagnostic::new("Cannot dereference non-pointer value")),
                    },
                    UnaryOp::Pos | UnaryOp::Neg | UnaryOp::Not => {
                        Ok(AsyncExprKind::Plain(BType::I32))
                    }
                }
            }
            Exp::BinaryExp(op, lhs, rhs) => {
                let lhs_kind = lhs.validate_semantics(ctx, loop_depth)?;
                let rhs_kind = rhs.validate_semantics(ctx, loop_depth)?;
                if lhs_kind.is_promise() || rhs_kind.is_promise() {
                    return Err(Diagnostic::new(
                        "Promise value cannot be used in expression without await",
                    ));
                }
                validate_binary_type(op, lhs_kind.plain_ty(), rhs_kind.plain_ty())?;
                Ok(AsyncExprKind::Plain(BType::I32))
            }
            Exp::ArrGet(lhs, rhs) => {
                let lhs_kind = lhs.validate_semantics(ctx, loop_depth)?;
                if lhs_kind.is_promise() {
                    return Err(Diagnostic::new(
                        "Promise value cannot be indexed without await",
                    ));
                }
                let rhs_kind = rhs.validate_semantics(ctx, loop_depth)?;
                if rhs_kind.is_promise() {
                    return Err(Diagnostic::new(
                        "Promise value cannot be used as an array index without await",
                    ));
                }
                let rhs_ty = rhs_kind.plain_ty().cloned().unwrap_or(BType::Void);
                expect_i32(&rhs_ty, "array index")?;
                Ok(array_element_type(lhs_kind.plain_ty())?)
            }
            Exp::Number(_) => Ok(AsyncExprKind::Plain(BType::I32)),
            Exp::Ident(name) => Ok(kind_from_btype(
                ctx.lookup_var(name)
                    .ok_or_else(|| format!("Unknown identifier {}", name))?,
            )),
            Exp::New(ty) => {
                validate_new_type_in_program(ty, ctx.program)?;
                Ok(AsyncExprKind::Plain(BType::Ptr(Box::new(ty.clone()))))
            }
        }
    }

    fn is_lvalue_shape(&self) -> bool {
        match self {
            Exp::Spanned(exp, _) => exp.is_lvalue_shape(),
            Exp::Ident(_) | Exp::ArrGet(_, _) | Exp::Field(_, _) | Exp::PtrField(_, _) => true,
            Exp::UnaryExp(UnaryOp::Deref, _) => true,
            Exp::Number(_)
            | Exp::UnaryExp(_, _)
            | Exp::BinaryExp(_, _, _)
            | Exp::New(_)
            | Exp::Await(_)
            | Exp::Sleep(_)
            | Exp::PromiseWait(_)
            | Exp::FuncCall(_, _) => false,
        }
    }
}

#[derive(Debug, Clone)]
enum AsyncExprKind {
    Plain(BType),
    Promise(BType),
}

impl AsyncExprKind {
    fn is_promise(&self) -> bool {
        matches!(self, AsyncExprKind::Promise(_))
    }

    fn plain_ty(&self) -> Option<&BType> {
        match self {
            AsyncExprKind::Plain(ty) => Some(ty),
            AsyncExprKind::Promise(_) => None,
        }
    }
}

struct SemanticCtx<'a> {
    program: &'a SemanticProgram,
    in_async_fn: bool,
    ret_ty: &'a BType,
    scopes: Vec<HashMap<String, BType>>,
    const_scopes: Vec<HashMap<String, i32>>,
    declared_promises: HashSet<String>,
    consumed_promises: HashSet<String>,
}

impl<'a> SemanticCtx<'a> {
    fn new(program: &'a SemanticProgram, in_async_fn: bool, ret_ty: &'a BType) -> Self {
        Self {
            program,
            in_async_fn,
            ret_ty,
            scopes: Vec::new(),
            const_scopes: Vec::new(),
            declared_promises: HashSet::new(),
            consumed_promises: HashSet::new(),
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
        self.const_scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
        self.const_scopes.pop();
    }

    fn define_var(&mut self, name: String, ty: BType) -> Result<(), String> {
        if let Some(scope) = self.scopes.last_mut() {
            if scope.contains_key(&name) {
                return Err(format!("Duplicate local definition: {}", name));
            }
            scope.insert(name.clone(), ty.clone());
        }
        if let BType::Promise(_) = ty {
            self.declared_promises.insert(name);
        }
        Ok(())
    }

    fn lookup_var(&self, name: &str) -> Option<BType> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
            .or_else(|| self.program.globals.get(name).cloned())
    }

    fn visible_constants(&self) -> HashMap<String, i32> {
        let mut constants = self.program.constants.clone();
        for scope in &self.const_scopes {
            constants.extend(scope.iter().map(|(name, value)| (name.clone(), *value)));
        }
        constants
    }

    fn define_const_from_decl(&mut self, ty: &Type, decl: &SingleDecl) {
        if !matches!(ty, Type::Const(BType::I32)) {
            return;
        }
        let (VarDecl::Ident(name), Some(init)) = (&decl.var, &decl.init) else {
            return;
        };
        let Some(exp) = init_exp(init) else {
            return;
        };
        let constants = self.visible_constants();
        if let Some(value) = eval_const_exp_with(exp, &|name| constants.get(name).copied()) {
            if let Some(scope) = self.const_scopes.last_mut() {
                scope.insert(name.clone(), value);
            }
        }
    }

    fn wait_promise_exp(&mut self, exp: &Exp) {
        if let Some(name) = exp_ident_name(exp) {
            self.consumed_promises.insert(name.clone());
        }
    }

    fn await_promise_exp(&mut self, exp: &Exp) {
        self.consume_promise_exp(exp);
    }

    fn consume_promise_exp(&mut self, exp: &Exp) {
        if let Some(name) = exp_ident_name(exp) {
            self.consumed_promises.insert(name.clone());
        }
    }

    fn finish_function(&self) -> Result<(), String> {
        for name in &self.declared_promises {
            if !self.consumed_promises.contains(name) {
                return Err(format!(
                    "Promise variable {} must be awaited or explicitly blocked with Promise.wait()",
                    name
                ));
            }
        }
        Ok(())
    }
}

fn exp_ident_name(exp: &Exp) -> Option<&String> {
    match exp {
        Exp::Spanned(exp, _) => exp_ident_name(exp),
        Exp::Ident(name) => Some(name),
        _ => None,
    }
}

fn type_base_btype(ty: &Type) -> &BType {
    match ty {
        Type::BType(btype) | Type::Const(btype) => btype,
    }
}

fn collect_global_constants(global_defs: &[GlobalDef]) -> HashMap<String, i32> {
    let mut constants = HashMap::new();
    let mut pending = Vec::new();
    for glob_def in global_defs {
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

fn init_exp(init: &InitVal) -> Option<&Exp> {
    match init {
        InitVal::Spanned(init, _) => init_exp(init),
        InitVal::Exp(exp) => Some(exp),
        InitVal::Arr(_) => None,
    }
}

fn var_decl_name(var: &VarDecl) -> String {
    match var {
        VarDecl::Ident(name) => name.clone(),
        VarDecl::Array(inner, _) => var_decl_name(inner),
    }
}

fn decl_btype(ty: &Type, var: &VarDecl, constants: &HashMap<String, i32>) -> Result<BType, String> {
    let mut base = type_base_btype(ty).clone();
    let mut dims = Vec::new();
    collect_decl_dims(var, constants, &mut dims)?;
    for dim in dims.into_iter().rev() {
        base = BType::Array(dim, Box::new(base));
    }
    Ok(base)
}

fn collect_decl_dims(
    var: &VarDecl,
    constants: &HashMap<String, i32>,
    dims: &mut Vec<usize>,
) -> Result<(), String> {
    match var {
        VarDecl::Ident(_) => Ok(()),
        VarDecl::Array(inner, len) => {
            collect_decl_dims(inner, constants, dims)?;
            let value = const_dim(len, constants, "Array length")?;
            dims.push(value);
            Ok(())
        }
    }
}

fn func_param_semantic_btype(
    param: &FuncParam,
    constants: &HashMap<String, i32>,
) -> Result<BType, String> {
    if let Some(dims) = &param.array_dims {
        let mut resolved = Vec::new();
        for dim in dims {
            resolved.push(const_dim(dim, constants, "Array parameter dimension")?);
        }
        Ok(func_param_btype(param.btype.clone(), resolved))
    } else {
        Ok(param.btype.clone())
    }
}

fn validate_return_type(ty: &BType, structs: &HashSet<String>) -> Result<(), String> {
    validate_type_refs(ty, structs)?;
    Ok(())
}

fn validate_object_type(
    ty: &BType,
    structs: &HashSet<String>,
    context: &str,
) -> Result<(), String> {
    validate_type_refs(ty, structs)?;
    if type_has_void_object(ty) {
        Err(format!("{} cannot have type {}", context, btype_name(ty)))
    } else {
        Ok(())
    }
}

fn validate_object_type_in_program(
    ty: &BType,
    program: &SemanticProgram,
    context: &str,
) -> Result<(), String> {
    let structs = program.structs.keys().cloned().collect::<HashSet<_>>();
    validate_object_type(ty, &structs, context)
}

fn validate_new_type_in_program(ty: &BType, program: &SemanticProgram) -> Result<(), String> {
    let structs = program.structs.keys().cloned().collect::<HashSet<_>>();
    validate_type_refs(ty, &structs)?;
    if matches!(ty, BType::Void) {
        Err("new cannot allocate void".to_string())
    } else {
        Ok(())
    }
}

fn validate_type_refs(ty: &BType, structs: &HashSet<String>) -> Result<(), String> {
    match ty {
        BType::Struct(name) if !structs.contains(name) => {
            Err(format!("Unknown struct type {}", name))
        }
        BType::Promise(inner) | BType::Ptr(inner) | BType::Array(_, inner) => {
            validate_type_refs(inner, structs)
        }
        BType::I32 | BType::Void | BType::Struct(_) => Ok(()),
    }
}

fn type_has_void_object(ty: &BType) -> bool {
    match ty {
        BType::Void => true,
        BType::Array(_, inner) => type_has_void_object(inner),
        BType::I32 | BType::Struct(_) | BType::Promise(_) | BType::Ptr(_) => false,
    }
}

fn validate_struct_value_cycles(structs: &HashMap<String, StructSig>) -> Result<(), Diagnostic> {
    let mut visiting = HashMap::new();
    let mut visited = HashSet::new();
    let mut names = structs.keys().cloned().collect::<Vec<_>>();
    names.sort();
    for name in names {
        let mut path = Vec::new();
        visit_struct_value_graph(&name, structs, &mut visiting, &mut visited, &mut path)?;
    }
    Ok(())
}

fn visit_struct_value_graph(
    name: &str,
    structs: &HashMap<String, StructSig>,
    visiting: &mut HashMap<String, usize>,
    visited: &mut HashSet<String>,
    path: &mut Vec<String>,
) -> Result<(), Diagnostic> {
    if visited.contains(name) {
        return Ok(());
    }
    visiting.insert(name.to_string(), path.len());
    path.push(name.to_string());
    let Some(sig) = structs.get(name) else {
        path.pop();
        visiting.remove(name);
        return Ok(());
    };
    for field in &sig.field_order {
        for dep in struct_value_dependencies(&field.ty) {
            if let Some(start) = visiting.get(&dep).copied() {
                let mut cycle = path[start..].to_vec();
                cycle.push(dep.clone());
                return Err(diagnostic_at(
                    field.span,
                    format!(
                        "Cyclic struct definition detected through field {}.{}: {}",
                        name,
                        field.name,
                        cycle.join(" -> ")
                    ),
                ));
            }
            visit_struct_value_graph(&dep, structs, visiting, visited, path)?;
        }
    }
    path.pop();
    visiting.remove(name);
    visited.insert(name.to_string());
    Ok(())
}

fn struct_value_dependencies(ty: &BType) -> Vec<String> {
    match ty {
        BType::Struct(name) => vec![name.clone()],
        BType::Array(_, inner) => struct_value_dependencies(inner),
        BType::I32 | BType::Void | BType::Promise(_) | BType::Ptr(_) => Vec::new(),
    }
}

fn requires_guaranteed_return(ty: &BType) -> bool {
    !matches!(ty, BType::Void)
}

fn block_guarantees_return(stmts: &[Stmt]) -> bool {
    stmts.iter().any(stmt_guarantees_return)
}

fn stmt_guarantees_return(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Spanned(stmt, _) => stmt_guarantees_return(stmt),
        Stmt::Return(_) => true,
        Stmt::Block(stmts) => block_guarantees_return(stmts),
        Stmt::IfElse(_, then_stmt, else_stmt) => {
            stmt_guarantees_return(then_stmt) && stmt_guarantees_return(else_stmt)
        }
        Stmt::Assign(_, _)
        | Stmt::Decl(_, _)
        | Stmt::Exp(_)
        | Stmt::If(_, _)
        | Stmt::While(_, _)
        | Stmt::PromiseWait(_)
        | Stmt::Continue
        | Stmt::Break
        | Stmt::Empty => false,
    }
}

fn call_arg_compatible(expected: &BType, actual: &BType) -> bool {
    btype_same(expected, actual)
        || matches!((expected, actual), (BType::Ptr(inner), BType::Array(_, actual_inner)) if btype_same(inner, actual_inner))
}

fn validate_call_arg_kind(
    func_name: &str,
    idx: usize,
    expected: &BType,
    actual: &AsyncExprKind,
) -> Result<(), String> {
    match (expected, actual) {
        (BType::Promise(expected), AsyncExprKind::Promise(actual)) => {
            ensure_promise_result_supported(expected)?;
            ensure_promise_result_supported(actual)?;
            if btype_same(expected, actual) {
                Ok(())
            } else {
                Err(format!(
                    "Function {} argument {} type mismatch: expected Promise<{}>, got Promise<{}>",
                    func_name,
                    idx + 1,
                    btype_name(expected),
                    btype_name(actual)
                ))
            }
        }
        (BType::Promise(expected), AsyncExprKind::Plain(actual)) => Err(format!(
            "Function {} argument {} type mismatch: expected Promise<{}>, got {}",
            func_name,
            idx + 1,
            btype_name(expected),
            btype_name(actual)
        )),
        (_, AsyncExprKind::Promise(_)) => {
            Err("Promise value cannot be passed as a normal argument without await".to_string())
        }
        (_, AsyncExprKind::Plain(actual)) => {
            if call_arg_compatible(expected, actual) {
                Ok(())
            } else {
                Err(format!(
                    "Function {} argument {} type mismatch: expected {}, got {}",
                    func_name,
                    idx + 1,
                    btype_name(expected),
                    btype_name(actual)
                ))
            }
        }
    }
}

fn assignment_compatible(expected: &BType, actual: &BType) -> bool {
    btype_same(expected, actual)
}

fn validate_assignment_kind(
    expected_kind: &AsyncExprKind,
    actual_kind: &AsyncExprKind,
) -> Result<(), String> {
    match (expected_kind, actual_kind) {
        (AsyncExprKind::Promise(expected), AsyncExprKind::Promise(actual)) => {
            if btype_same(expected, actual) {
                Ok(())
            } else {
                Err(format!(
                    "Promise assignment type mismatch: expected {}, got {}",
                    btype_name(expected),
                    btype_name(actual)
                ))
            }
        }
        (AsyncExprKind::Plain(_), AsyncExprKind::Promise(_)) => {
            Err("Promise value cannot be assigned without await".to_string())
        }
        (AsyncExprKind::Promise(_), AsyncExprKind::Plain(_)) => {
            Err("Promise variable must be assigned a Promise value".to_string())
        }
        (AsyncExprKind::Plain(expected), AsyncExprKind::Plain(actual)) => {
            if matches!(expected, BType::Array(_, _)) || matches!(actual, BType::Array(_, _)) {
                return Err("Array values cannot be assigned".to_string());
            }
            if assignment_compatible(expected, actual) {
                Ok(())
            } else {
                Err(format!(
                    "Assignment type mismatch: expected {}, got {}",
                    btype_name(expected),
                    btype_name(actual)
                ))
            }
        }
    }
}

fn validate_initializer_kind(expected: &BType, actual: &AsyncExprKind) -> Result<(), String> {
    match (expected, actual) {
        (BType::Promise(expected), AsyncExprKind::Promise(actual)) => {
            ensure_promise_result_supported(expected)?;
            ensure_promise_result_supported(actual)?;
            if btype_same(expected, actual) {
                Ok(())
            } else {
                Err(format!(
                    "Promise initializer type mismatch: expected Promise<{}>, got Promise<{}>",
                    btype_name(expected),
                    btype_name(actual)
                ))
            }
        }
        (BType::Promise(_), AsyncExprKind::Plain(_)) => {
            Err("Promise variable must be initialized with a Promise value".to_string())
        }
        (_, AsyncExprKind::Promise(_)) => {
            Err("Promise value cannot be used in initializer without await".to_string())
        }
        (_, AsyncExprKind::Plain(actual)) => {
            if matches!(actual, BType::Array(_, _)) {
                return Err("Array value cannot be used as an initializer expression".to_string());
            }
            if assignment_compatible(expected, actual) {
                Ok(())
            } else {
                Err(format!(
                    "Initializer type mismatch: expected {}, got {}",
                    btype_name(expected),
                    btype_name(actual)
                ))
            }
        }
    }
}

fn array_element_type(ty: Option<&BType>) -> Result<AsyncExprKind, String> {
    match ty {
        Some(BType::Array(_, inner)) | Some(BType::Ptr(inner)) => {
            Ok(kind_from_btype((**inner).clone()))
        }
        Some(other) => Err(format!("Cannot index into {}", btype_name(other))),
        None => Err("Cannot index into non-value expression".to_string()),
    }
}

fn field_access_type(
    program: &SemanticProgram,
    base: Option<&BType>,
    field: &str,
) -> Result<AsyncExprKind, String> {
    match base {
        Some(BType::Struct(name)) => {
            let sig = program
                .structs
                .get(name)
                .ok_or_else(|| format!("Unknown struct type {}", name))?;
            let ty = sig
                .fields
                .get(field)
                .cloned()
                .ok_or_else(|| format!("Struct {} has no field {}", name, field))?;
            Ok(kind_from_btype(ty))
        }
        Some(other) => Err(format!("Cannot access field on {}", btype_name(other))),
        None => Err("Cannot access field on non-value expression".to_string()),
    }
}

fn ptr_field_access_type(
    program: &SemanticProgram,
    base: Option<&BType>,
    field: &str,
) -> Result<AsyncExprKind, String> {
    match base {
        Some(BType::Ptr(inner)) => field_access_type(program, Some(inner), field),
        Some(other) => Err(format!(
            "Cannot access pointer field through {}",
            btype_name(other)
        )),
        None => Err("Cannot access pointer field through non-value expression".to_string()),
    }
}

fn validate_unary_type(op: &UnaryOp, ty: Option<&BType>) -> Result<(), String> {
    match op {
        UnaryOp::Pos | UnaryOp::Neg | UnaryOp::Not => expect_i32_option(ty, "unary expression"),
        UnaryOp::Addr => {
            if ty.is_some() {
                Ok(())
            } else {
                Err("Cannot take address of non-value expression".to_string())
            }
        }
        UnaryOp::Deref => match ty {
            Some(BType::Ptr(_)) => Ok(()),
            Some(other) => Err(format!("Cannot dereference {}", btype_name(other))),
            None => Err("Cannot dereference non-value expression".to_string()),
        },
    }
}

fn validate_address_operand(exp: &Exp) -> Result<(), String> {
    if exp.is_lvalue_shape() {
        Ok(())
    } else {
        Err("Cannot take address of non-lvalue expression".to_string())
    }
}

fn ensure_promise_result_supported(ty: &BType) -> Result<(), String> {
    if type_contains_array(ty) {
        Err(format!(
            "Promise result type {} is not supported because arrays are not first-class values",
            btype_name(ty)
        ))
    } else {
        Ok(())
    }
}

fn type_contains_array(ty: &BType) -> bool {
    match ty {
        BType::Array(_, _) => true,
        BType::Promise(inner) => type_contains_array(inner),
        BType::I32 | BType::Void | BType::Struct(_) | BType::Ptr(_) => false,
    }
}

fn validate_binary_type(
    _op: &BinaryOp,
    lhs: Option<&BType>,
    rhs: Option<&BType>,
) -> Result<(), String> {
    expect_i32_option(lhs, "left operand")?;
    expect_i32_option(rhs, "right operand")
}

fn expect_i32(ty: &BType, context: &str) -> Result<(), String> {
    if matches!(ty, BType::I32) {
        Ok(())
    } else {
        Err(format!("{} expects int, got {}", context, btype_name(ty)))
    }
}

fn expect_i32_option(ty: Option<&BType>, context: &str) -> Result<(), String> {
    match ty {
        Some(ty) => expect_i32(ty, context),
        None => Err(format!("{} expects int", context)),
    }
}

fn validate_initializer(
    ctx: &mut SemanticCtx<'_>,
    init: &InitVal,
    expected: &BType,
    loop_depth: usize,
) -> Result<(), Diagnostic> {
    match init {
        InitVal::Spanned(init, span) => validate_initializer(ctx, init, expected, loop_depth)
            .map_err(|err| err.with_fallback_span(*span)),
        InitVal::Exp(exp) => {
            if matches!(expected, BType::Array(_, _)) {
                return Err(Diagnostic::new(
                    "Array initializer must use an initializer list",
                ));
            }
            let kind = exp.validate_semantics(ctx, loop_depth)?;
            validate_initializer_kind(expected, &kind).map_err(Diagnostic::from)
        }
        InitVal::Arr(items) => match expected {
            BType::Array(_, inner) => {
                let capacity = array_scalar_capacity(expected);
                let used = initializer_scalar_count(init);
                if used > capacity {
                    return Err(Diagnostic::new(format!(
                        "Too many initializer values for {}",
                        btype_name(expected)
                    )));
                }
                for item in items {
                    if matches!(unspan_init(item), InitVal::Arr(_)) {
                        validate_initializer(ctx, item, inner, loop_depth)?;
                    } else {
                        validate_initializer(ctx, item, inner_scalar_btype(expected), loop_depth)?;
                    }
                }
                Ok(())
            }
            BType::Struct(name) => {
                let fields = ctx
                    .program
                    .structs
                    .get(name)
                    .ok_or_else(|| Diagnostic::new(format!("Unknown struct type {}", name)))?
                    .field_order
                    .clone();
                for (idx, item) in items.iter().enumerate() {
                    let Some(field_ty) = fields.get(idx) else {
                        return Err(Diagnostic::new(format!(
                            "Too many initializer values for {}",
                            name
                        )));
                    };
                    validate_initializer(ctx, item, &field_ty.ty, loop_depth)?;
                }
                Ok(())
            }
            other => Err(Diagnostic::new(format!(
                "Initializer list cannot initialize {}",
                btype_name(other)
            ))),
        },
    }
}

fn initializer_scalar_count(init: &InitVal) -> usize {
    match init {
        InitVal::Spanned(init, _) => initializer_scalar_count(init),
        InitVal::Exp(_) => 1,
        InitVal::Arr(items) => items.iter().map(initializer_scalar_count).sum(),
    }
}

fn unspan_init(init: &InitVal) -> &InitVal {
    match init {
        InitVal::Spanned(init, _) => unspan_init(init),
        other => other,
    }
}

fn array_scalar_capacity(ty: &BType) -> usize {
    match ty {
        BType::Array(len, inner) => len * array_scalar_capacity(inner),
        _ => 1,
    }
}

fn inner_scalar_btype(ty: &BType) -> &BType {
    match ty {
        BType::Array(_, inner) => inner_scalar_btype(inner),
        other => other,
    }
}

fn const_dim(exp: &Exp, constants: &HashMap<String, i32>, context: &str) -> Result<usize, String> {
    let value = eval_const_exp_with(exp, &|name| constants.get(name).copied())
        .ok_or_else(|| format!("{} must be a constant expression", context))?;
    if value < 0 {
        return Err(format!("{} must be non-negative", context));
    }
    Ok(value as usize)
}

fn kind_from_btype(ty: BType) -> AsyncExprKind {
    match ty {
        BType::Promise(inner) => AsyncExprKind::Promise(*inner),
        other => AsyncExprKind::Plain(other),
    }
}

fn btype_same(lhs: &BType, rhs: &BType) -> bool {
    match (lhs, rhs) {
        (BType::I32, BType::I32) | (BType::Void, BType::Void) => true,
        (BType::Struct(lhs), BType::Struct(rhs)) => lhs == rhs,
        (BType::Promise(lhs), BType::Promise(rhs)) | (BType::Ptr(lhs), BType::Ptr(rhs)) => {
            btype_same(lhs, rhs)
        }
        (BType::Array(lhs_len, lhs), BType::Array(rhs_len, rhs)) => {
            lhs_len == rhs_len && btype_same(lhs, rhs)
        }
        _ => false,
    }
}

fn btype_name(ty: &BType) -> String {
    match ty {
        BType::I32 => "int".to_string(),
        BType::Void => "void".to_string(),
        BType::Struct(name) => name.clone(),
        BType::Promise(inner) => format!("Promise<{}>", btype_name(inner)),
        BType::Ptr(inner) => format!("{}*", btype_name(inner)),
        BType::Array(len, inner) => format!("{}[{}]", btype_name(inner), len),
    }
}

fn builtin_function_sigs() -> HashMap<String, FunctionSig> {
    let mut funcs = HashMap::new();
    let ptr_i32 = BType::Ptr(Box::new(BType::I32));
    for (name, ret, params) in [
        ("getint", BType::I32, vec![]),
        ("getch", BType::I32, vec![]),
        ("getarray", BType::I32, vec![ptr_i32.clone()]),
        ("putint", BType::Void, vec![BType::I32]),
        ("putch", BType::I32, vec![BType::I32]),
        ("putarray", BType::Void, vec![BType::I32, ptr_i32]),
        ("starttime", BType::Void, vec![]),
        ("stoptime", BType::Void, vec![]),
    ] {
        funcs.insert(
            name.to_string(),
            FunctionSig {
                is_async: false,
                ret,
                param_count: params.len(),
                params,
            },
        );
    }
    funcs
}

fn reject_unsupported_await_position(exp: &Exp, context: &str) -> Result<(), String> {
    if exp_contains_await(exp) {
        Err(format!("await inside {} is not supported", context))
    } else {
        Ok(())
    }
}

fn init_contains_await(init: &InitVal) -> bool {
    match init {
        InitVal::Spanned(init, _) => init_contains_await(init),
        InitVal::Exp(exp) => exp_contains_await(exp),
        InitVal::Arr(items) => items.iter().any(init_contains_await),
    }
}

fn exp_contains_await(exp: &Exp) -> bool {
    match exp {
        Exp::Spanned(exp, _) => exp_contains_await(exp),
        Exp::Await(_) => true,
        Exp::UnaryExp(_, exp)
        | Exp::Field(exp, _)
        | Exp::PtrField(exp, _)
        | Exp::Sleep(exp)
        | Exp::PromiseWait(exp) => exp_contains_await(exp),
        Exp::BinaryExp(_, lhs, rhs) | Exp::ArrGet(lhs, rhs) => {
            exp_contains_await(lhs) || exp_contains_await(rhs)
        }
        Exp::FuncCall(_, args) => args.iter().any(exp_contains_await),
        Exp::Number(_) | Exp::New(_) | Exp::Ident(_) => false,
    }
}

struct AsyncFunctionAnalyzer {
    next_continuation_id: usize,
    await_points: Vec<AwaitPoint>,
    frame_local_candidates: HashSet<String>,
    declared_before: HashSet<String>,
}

impl AsyncFunctionAnalyzer {
    fn new() -> Self {
        Self {
            next_continuation_id: 1,
            await_points: Vec::new(),
            frame_local_candidates: HashSet::new(),
            declared_before: HashSet::new(),
        }
    }

    fn visit_stmts(&mut self, stmts: &[Stmt], in_loop: bool) {
        for (idx, stmt) in stmts.iter().enumerate() {
            self.visit_stmt(stmt, in_loop, &stmts[idx + 1..]);
        }
    }

    fn visit_stmt(&mut self, stmt: &Stmt, in_loop: bool, following: &[Stmt]) {
        match stmt {
            Stmt::Spanned(stmt, _) => self.visit_stmt(stmt, in_loop, following),
            Stmt::Block(stmts) => {
                self.visit_stmts(stmts, in_loop);
            }
            Stmt::Assign(lhs, rhs) => {
                self.visit_exp(lhs, in_loop, following);
                self.visit_exp(rhs, in_loop, following);
            }
            Stmt::Decl(_, decls) => {
                for decl in decls {
                    let name = var_decl_name(&decl.var);
                    if decl.init.as_ref().is_some_and(init_contains_await) {
                        self.declared_before.insert(name.clone());
                    }
                    if let Some(init) = &decl.init {
                        self.visit_init(init, in_loop, following);
                    }
                    self.declared_before.insert(name);
                }
            }
            Stmt::Exp(exp) | Stmt::Return(Some(exp)) | Stmt::PromiseWait(exp) => {
                self.visit_exp(exp, in_loop, following);
            }
            Stmt::If(cond, then_stmt) => {
                self.visit_exp(cond, in_loop, following);
                self.visit_stmt(then_stmt, in_loop, following);
            }
            Stmt::IfElse(cond, then_stmt, else_stmt) => {
                self.visit_exp(cond, in_loop, following);
                self.visit_stmt(then_stmt, in_loop, following);
                self.visit_stmt(else_stmt, in_loop, following);
            }
            Stmt::While(cond, body) => {
                self.visit_exp(cond, true, following);
                self.visit_stmt(body, true, following);
            }
            Stmt::Continue | Stmt::Break | Stmt::Return(None) | Stmt::Empty => {}
        }
    }

    fn visit_init(&mut self, init: &InitVal, in_loop: bool, following: &[Stmt]) {
        match init {
            InitVal::Spanned(init, _) => self.visit_init(init, in_loop, following),
            InitVal::Exp(exp) => self.visit_exp(exp, in_loop, following),
            InitVal::Arr(items) => {
                for item in items {
                    self.visit_init(item, in_loop, following);
                }
            }
        }
    }

    fn visit_exp(&mut self, exp: &Exp, in_loop: bool, following: &[Stmt]) {
        match exp {
            Exp::Spanned(exp, _) => self.visit_exp(exp, in_loop, following),
            Exp::Await(inner) => {
                let continuation_id = self.next_continuation_id;
                self.next_continuation_id += 1;
                self.await_points.push(AwaitPoint {
                    continuation_id,
                    in_loop,
                });
                let used_after = collect_used_vars_in_stmts(following);
                for name in self.declared_before.intersection(&used_after) {
                    self.frame_local_candidates.insert(name.clone());
                }
                self.visit_exp(inner, in_loop, following);
            }
            Exp::Sleep(duration) | Exp::PromiseWait(duration) => {
                self.visit_exp(duration, in_loop, following)
            }
            Exp::UnaryExp(_, exp) => self.visit_exp(exp, in_loop, following),
            Exp::BinaryExp(_, lhs, rhs) | Exp::ArrGet(lhs, rhs) => {
                self.visit_exp(lhs, in_loop, following);
                self.visit_exp(rhs, in_loop, following);
            }
            Exp::FuncCall(_, args) => {
                for arg in args {
                    self.visit_exp(arg, in_loop, following);
                }
            }
            Exp::Field(base, _) | Exp::PtrField(base, _) => {
                self.visit_exp(base, in_loop, following);
            }
            Exp::Number(_) | Exp::New(_) | Exp::Ident(_) => {}
        }
    }
}

fn collect_used_vars_in_stmts(stmts: &[Stmt]) -> HashSet<String> {
    let mut vars = HashSet::new();
    for stmt in stmts {
        collect_used_vars_in_stmt(stmt, &mut vars);
    }
    vars
}

fn collect_used_vars_in_stmt(stmt: &Stmt, vars: &mut HashSet<String>) {
    match stmt {
        Stmt::Spanned(stmt, _) => collect_used_vars_in_stmt(stmt, vars),
        Stmt::Block(stmts) => {
            for stmt in stmts {
                collect_used_vars_in_stmt(stmt, vars);
            }
        }
        Stmt::Assign(lhs, rhs) => {
            collect_used_vars_in_exp(lhs, vars);
            collect_used_vars_in_exp(rhs, vars);
        }
        Stmt::Decl(_, decls) => {
            for decl in decls {
                if let Some(init) = &decl.init {
                    collect_used_vars_in_init(init, vars);
                }
            }
        }
        Stmt::Exp(exp) | Stmt::Return(Some(exp)) | Stmt::PromiseWait(exp) => {
            collect_used_vars_in_exp(exp, vars);
        }
        Stmt::If(cond, then_stmt) => {
            collect_used_vars_in_exp(cond, vars);
            collect_used_vars_in_stmt(then_stmt, vars);
        }
        Stmt::IfElse(cond, then_stmt, else_stmt) => {
            collect_used_vars_in_exp(cond, vars);
            collect_used_vars_in_stmt(then_stmt, vars);
            collect_used_vars_in_stmt(else_stmt, vars);
        }
        Stmt::While(cond, body) => {
            collect_used_vars_in_exp(cond, vars);
            collect_used_vars_in_stmt(body, vars);
        }
        Stmt::Continue | Stmt::Break | Stmt::Return(None) | Stmt::Empty => {}
    }
}

fn collect_used_vars_in_init(init: &InitVal, vars: &mut HashSet<String>) {
    match init {
        InitVal::Spanned(init, _) => collect_used_vars_in_init(init, vars),
        InitVal::Exp(exp) => collect_used_vars_in_exp(exp, vars),
        InitVal::Arr(items) => {
            for item in items {
                collect_used_vars_in_init(item, vars);
            }
        }
    }
}

fn collect_used_vars_in_exp(exp: &Exp, vars: &mut HashSet<String>) {
    match exp {
        Exp::Spanned(exp, _) => collect_used_vars_in_exp(exp, vars),
        Exp::Ident(name) => {
            vars.insert(name.clone());
        }
        Exp::Await(inner)
        | Exp::Sleep(inner)
        | Exp::PromiseWait(inner)
        | Exp::UnaryExp(_, inner)
        | Exp::Field(inner, _)
        | Exp::PtrField(inner, _) => collect_used_vars_in_exp(inner, vars),
        Exp::BinaryExp(_, lhs, rhs) | Exp::ArrGet(lhs, rhs) => {
            collect_used_vars_in_exp(lhs, vars);
            collect_used_vars_in_exp(rhs, vars);
        }
        Exp::FuncCall(_, args) => {
            for arg in args {
                collect_used_vars_in_exp(arg, vars);
            }
        }
        Exp::Number(_) | Exp::New(_) => {}
    }
}

fn type_contains_promise(ty: &Type) -> bool {
    match ty {
        Type::BType(btype) | Type::Const(btype) => btype_contains_promise(btype),
    }
}

fn btype_contains_promise(ty: &BType) -> bool {
    match ty {
        BType::Promise(_) => true,
        BType::Ptr(inner) | BType::Array(_, inner) => btype_contains_promise(inner),
        BType::I32 | BType::Void | BType::Struct(_) => false,
    }
}

#[derive(Debug, Clone)]
pub enum UnaryOp {
    Pos,
    Neg,
    Not,
    Addr,
    Deref,
}

#[derive(Debug, Clone)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Lt,
    Gt,
    Le,
    Ge,
    Eq,
    Ne,
    And,
    Or,
}
