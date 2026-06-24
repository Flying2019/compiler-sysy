use std::collections::{HashMap, HashSet};

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
}

#[derive(Debug, Clone)]
struct SemanticProgram {
    funcs: HashMap<String, FunctionSig>,
    structs: HashMap<String, StructSig>,
}

#[derive(Debug, Clone)]
pub enum GlobalDef {
    FuncDef(FuncDef),
    GlobalDecl(Type, Vec<SingleDecl>), // @global = decl type, init
    StructDef(StructDef),
}

#[derive(Debug, Clone)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<StructField>,
}

#[derive(Debug, Clone)]
pub struct StructField {
    pub ty: Type,
    pub decls: Vec<SingleDecl>,
}

#[derive(Debug, Clone)]
pub struct FuncDef {
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

#[derive(Debug, Clone)]
pub enum VarDecl {
    Ident(String),
    Array(Box<VarDecl>, Exp),
}

#[derive(Debug, Clone)]
pub struct SingleDecl {
    pub var: VarDecl,
    pub init: Option<InitVal>,
}

#[derive(Debug, Clone)]
pub enum InitVal {
    Exp(Exp),
    Arr(Vec<InitVal>),
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
                UnaryOp::Addr | UnaryOp::Deref => unreachable!(),
            })
        }
        Exp::BinaryExp(op, lhs, rhs) => {
            let lhs = eval_const_exp_with(lhs, lookup)?;
            let rhs = eval_const_exp_with(rhs, lookup)?;
            Some(match op {
                BinaryOp::Add => lhs + rhs,
                BinaryOp::Sub => lhs - rhs,
                BinaryOp::Mul => lhs * rhs,
                BinaryOp::Div => lhs / rhs,
                BinaryOp::Mod => lhs % rhs,
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

pub fn eval_param_dim<F>(exp: &Exp, lookup: &F) -> usize
where
    F: Fn(&str) -> Option<i32>,
{
    let value = eval_const_exp_with(exp, lookup)
        .expect("Array parameter dimensions must be constant expressions");
    assert!(
        value >= 0,
        "Array parameter dimensions must be non-negative"
    );
    value as usize
}

#[derive(Debug, Clone)]
pub enum Exp {
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

impl CompUnit {
    pub fn contains_async_syntax(&self) -> bool {
        self.global_defs
            .iter()
            .any(GlobalDef::contains_async_syntax)
    }

    pub fn validate_async_syntax(&self) -> Result<(), String> {
        let funcs = self.async_function_sigs()?;
        for glob_def in &self.global_defs {
            glob_def.validate_async_syntax(&funcs)?;
        }
        Ok(())
    }

    pub fn validate_semantics(&self) -> Result<(), String> {
        self.validate_async_syntax()?;
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

    fn async_function_sigs(&self) -> Result<HashMap<String, AsyncFunctionSig>, String> {
        let mut funcs = HashMap::new();
        for glob_def in &self.global_defs {
            if let GlobalDef::FuncDef(func) = glob_def {
                if funcs
                    .insert(
                        func.ident.clone(),
                        AsyncFunctionSig {
                            is_async: func.is_async,
                            ret: type_base_btype(&func.func_type).clone(),
                        },
                    )
                    .is_some()
                {
                    return Err(format!("Duplicate function definition: {}", func.ident));
                }
            }
        }
        Ok(funcs)
    }

    fn semantic_program(&self) -> Result<SemanticProgram, String> {
        let mut funcs = builtin_function_sigs();
        let mut structs = HashMap::new();
        for glob_def in &self.global_defs {
            if let GlobalDef::StructDef(def) = glob_def {
                let mut fields = HashMap::new();
                for field in &def.fields {
                    for decl in &field.decls {
                        fields.insert(var_decl_name(&decl.var), decl_btype(&field.ty, &decl.var));
                    }
                }
                structs.insert(def.name.clone(), StructSig { fields });
            }
        }
        for glob_def in &self.global_defs {
            if let GlobalDef::FuncDef(func) = glob_def {
                if funcs
                    .insert(
                        func.ident.clone(),
                        FunctionSig {
                            is_async: func.is_async,
                            ret: type_base_btype(&func.func_type).clone(),
                            param_count: func.func_params.len(),
                            params: func
                                .func_params
                                .iter()
                                .map(func_param_semantic_btype)
                                .collect(),
                        },
                    )
                    .is_some()
                {
                    return Err(format!("Duplicate function definition: {}", func.ident));
                }
            }
        }
        Ok(SemanticProgram { funcs, structs })
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

    fn validate_async_syntax(
        &self,
        funcs: &HashMap<String, AsyncFunctionSig>,
    ) -> Result<(), String> {
        match self {
            GlobalDef::FuncDef(func) => func.validate_async_syntax(funcs),
            GlobalDef::GlobalDecl(_, decls) => {
                let mut ctx = AsyncValidationContext::new(funcs, false);
                for decl in decls {
                    if let Some(init) = &decl.init {
                        init.validate_async_syntax(&mut ctx)?;
                    }
                }
                Ok(())
            }
            GlobalDef::StructDef(_) => Ok(()),
        }
    }

    fn validate_semantics(&self, program: &SemanticProgram) -> Result<(), String> {
        match self {
            GlobalDef::FuncDef(func) => func.validate_semantics(program),
            GlobalDef::GlobalDecl(_, decls) => {
                let mut ctx = SemanticCtx::new(program, false, &BType::Void);
                for decl in decls {
                    if let Some(init) = &decl.init {
                        init.validate_semantics(&mut ctx, 0)?;
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

    fn validate_async_syntax(
        &self,
        funcs: &HashMap<String, AsyncFunctionSig>,
    ) -> Result<(), String> {
        let mut ctx = AsyncValidationContext::new(funcs, self.is_async);
        ctx.push_scope();
        for param in &self.func_params {
            if let BType::Promise(inner) = &param.btype {
                ctx.define_promise(param.name.clone(), (**inner).clone());
            }
        }
        for stmt in &self.block {
            stmt.validate_async_syntax(&mut ctx)?;
        }
        ctx.finish_function()?;
        Ok(())
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

    fn validate_semantics(&self, program: &SemanticProgram) -> Result<(), String> {
        let ret_ty = type_base_btype(&self.func_type);
        let mut ctx = SemanticCtx::new(program, self.is_async, ret_ty);
        ctx.push_scope();
        for param in &self.func_params {
            ctx.define_var(param.name.clone(), func_param_semantic_btype(param));
        }
        for stmt in &self.block {
            stmt.validate_semantics(&mut ctx, 0)?;
        }
        ctx.pop_scope();
        Ok(())
    }
}

impl Stmt {
    fn contains_async_syntax(&self) -> bool {
        match self {
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

    fn validate_async_syntax(&self, ctx: &mut AsyncValidationContext<'_>) -> Result<(), String> {
        match self {
            Stmt::Block(stmts) => {
                ctx.push_scope();
                for stmt in stmts {
                    stmt.validate_async_syntax(ctx)?;
                }
                ctx.pop_scope();
                Ok(())
            }
            Stmt::Assign(lhs, rhs) => {
                reject_unsupported_await_position(lhs, "assignment target")?;
                reject_unsupported_await_nested(rhs, "assignment value")?;
                if matches!(rhs, Exp::Await(_)) && !matches!(lhs, Exp::Ident(_)) {
                    return Err("await assignment target must be a simple identifier".to_string());
                }
                lhs.validate_async_syntax(ctx)?;
                let rhs_kind = rhs.validate_async_syntax(ctx)?;
                if rhs_kind.is_promise() {
                    return Err("Promise value cannot be assigned without await".to_string());
                }
                Ok(())
            }
            Stmt::Decl(ty, decls) => {
                let declared_promise = match type_base_btype(ty) {
                    BType::Promise(inner) => Some((**inner).clone()),
                    _ => None,
                };
                for decl in decls {
                    let var_name = var_decl_name(&decl.var);
                    if let Some(init) = &decl.init {
                        reject_unsupported_await_nested_in_init(init, "declaration initializer")?;
                        let init_kind = init.validate_async_syntax(ctx)?;
                        match (&declared_promise, init_kind) {
                            (Some(expected), AsyncExprKind::Promise(actual)) => {
                                if !btype_same(expected, &actual) {
                                    return Err(format!(
                                        "Promise initializer type mismatch for {}",
                                        var_name
                                    ));
                                }
                            }
                            (Some(_), AsyncExprKind::Plain(_)) => {
                                return Err(format!(
                                    "Promise variable {} must be initialized with a Promise value",
                                    var_name
                                ));
                            }
                            (None, AsyncExprKind::Promise(_)) => {
                                return Err(format!(
                                    "Promise value cannot initialize non-Promise variable {} without await",
                                    var_name
                                ));
                            }
                            (None, AsyncExprKind::Plain(_)) => {}
                        }
                    }
                    if let Some(inner) = &declared_promise {
                        ctx.define_promise(var_name, inner.clone());
                    }
                }
                Ok(())
            }
            Stmt::Exp(exp) => {
                reject_unsupported_await_nested(exp, "expression statement")?;
                let kind = exp.validate_async_syntax(ctx)?;
                if kind.is_promise() {
                    return Err(
                        "Promise expression must be awaited, stored, or registered".to_string()
                    );
                }
                Ok(())
            }
            Stmt::Return(Some(exp)) => {
                reject_unsupported_await_nested(exp, "return expression")?;
                exp.validate_async_syntax(ctx)?;
                Ok(())
            }
            Stmt::PromiseWait(exp) => {
                reject_unsupported_await_position(exp, "Promise.wait receiver")?;
                let kind = exp.validate_async_syntax(ctx)?;
                if !kind.is_promise() {
                    return Err("Promise.wait() expects a Promise value".to_string());
                }
                ctx.wait_promise_exp(exp);
                Ok(())
            }
            Stmt::If(cond, then_stmt) => {
                reject_unsupported_await_position(cond, "if condition")?;
                if cond.validate_async_syntax(ctx)?.is_promise() {
                    return Err("Promise value cannot be used as an if condition".to_string());
                }
                then_stmt.validate_async_syntax(ctx)
            }
            Stmt::IfElse(cond, then_stmt, else_stmt) => {
                reject_unsupported_await_position(cond, "if condition")?;
                if cond.validate_async_syntax(ctx)?.is_promise() {
                    return Err("Promise value cannot be used as an if condition".to_string());
                }
                then_stmt.validate_async_syntax(ctx)?;
                else_stmt.validate_async_syntax(ctx)
            }
            Stmt::While(cond, body) => {
                reject_unsupported_await_position(cond, "while condition")?;
                if body.contains_async_syntax() {
                    return Err(
                        "await inside while body is not supported by callback-hole lowering yet"
                            .to_string(),
                    );
                }
                if cond.validate_async_syntax(ctx)?.is_promise() {
                    return Err("Promise value cannot be used as a while condition".to_string());
                }
                body.validate_async_syntax(ctx)
            }
            Stmt::Continue | Stmt::Break | Stmt::Return(None) | Stmt::Empty => Ok(()),
        }
    }

    fn validate_semantics(
        &self,
        ctx: &mut SemanticCtx<'_>,
        loop_depth: usize,
    ) -> Result<(), String> {
        match self {
            Stmt::Block(stmts) => {
                ctx.push_scope();
                for stmt in stmts {
                    stmt.validate_semantics(ctx, loop_depth)?;
                }
                ctx.pop_scope();
                Ok(())
            }
            Stmt::Assign(lhs, rhs) => {
                lhs.validate_semantics(ctx, loop_depth)?;
                rhs.validate_semantics(ctx, loop_depth)?;
                Ok(())
            }
            Stmt::Decl(ty, decls) => {
                for decl in decls {
                    if let Some(init) = &decl.init {
                        init.validate_semantics(ctx, loop_depth)?;
                    }
                    ctx.define_var(var_decl_name(&decl.var), decl_btype(ty, &decl.var));
                }
                Ok(())
            }
            Stmt::Exp(exp) | Stmt::PromiseWait(exp) => {
                exp.validate_semantics(ctx, loop_depth)?;
                Ok(())
            }
            Stmt::Return(Some(exp)) => {
                let kind = exp.validate_semantics(ctx, loop_depth)?;
                if kind.is_promise() {
                    return Err("Promise value cannot be returned without await".to_string());
                }
                let actual_ty = kind.plain_ty().unwrap_or(&BType::Void);
                if matches!(ctx.ret_ty, BType::Void) {
                    if !matches!(actual_ty, BType::Void) {
                        return Err("Void function cannot return a value".to_string());
                    }
                } else if !btype_same(ctx.ret_ty, actual_ty) {
                    return Err(format!(
                        "Return type mismatch: expected {}, got {}",
                        btype_name(ctx.ret_ty),
                        btype_name(actual_ty)
                    ));
                }
                Ok(())
            }
            Stmt::If(cond, then_stmt) => {
                cond.validate_semantics(ctx, loop_depth)?;
                then_stmt.validate_semantics(ctx, loop_depth)
            }
            Stmt::IfElse(cond, then_stmt, else_stmt) => {
                cond.validate_semantics(ctx, loop_depth)?;
                then_stmt.validate_semantics(ctx, loop_depth)?;
                else_stmt.validate_semantics(ctx, loop_depth)
            }
            Stmt::While(cond, body) => {
                cond.validate_semantics(ctx, loop_depth)?;
                body.validate_semantics(ctx, loop_depth + 1)
            }
            Stmt::Break | Stmt::Continue if loop_depth == 0 => {
                Err("break/continue used outside a loop".to_string())
            }
            Stmt::Return(None) if !matches!(ctx.ret_ty, BType::Void) => Err(format!(
                "Function returning {} cannot use empty return",
                btype_name(ctx.ret_ty)
            )),
            Stmt::Break | Stmt::Continue | Stmt::Return(None) | Stmt::Empty => Ok(()),
        }
    }
}

impl InitVal {
    fn contains_async_syntax(&self) -> bool {
        match self {
            InitVal::Exp(exp) => exp.contains_async_syntax(),
            InitVal::Arr(items) => items.iter().any(InitVal::contains_async_syntax),
        }
    }

    fn validate_async_syntax(
        &self,
        ctx: &mut AsyncValidationContext<'_>,
    ) -> Result<AsyncExprKind, String> {
        match self {
            InitVal::Exp(exp) => exp.validate_async_syntax(ctx),
            InitVal::Arr(items) => {
                for item in items {
                    if item.validate_async_syntax(ctx)?.is_promise() {
                        return Err(
                            "Promise value cannot be used inside array initializer".to_string()
                        );
                    }
                }
                Ok(AsyncExprKind::Plain(BType::I32))
            }
        }
    }

    fn validate_semantics(
        &self,
        ctx: &mut SemanticCtx<'_>,
        loop_depth: usize,
    ) -> Result<AsyncExprKind, String> {
        match self {
            InitVal::Exp(exp) => exp.validate_semantics(ctx, loop_depth),
            InitVal::Arr(items) => {
                for item in items {
                    item.validate_semantics(ctx, loop_depth)?;
                }
                Ok(AsyncExprKind::Plain(BType::I32))
            }
        }
    }
}

impl Exp {
    fn contains_async_syntax(&self) -> bool {
        match self {
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

    fn validate_async_syntax(
        &self,
        ctx: &mut AsyncValidationContext<'_>,
    ) -> Result<AsyncExprKind, String> {
        match self {
            Exp::Await(inner) => {
                if !ctx.in_async_fn {
                    return Err("await is only allowed inside async functions".to_string());
                }
                let kind = inner.validate_async_syntax(ctx)?;
                let AsyncExprKind::Promise(inner_ty) = kind else {
                    return Err("await expects a Promise value".to_string());
                };
                ctx.await_promise_exp(inner);
                Ok(AsyncExprKind::Plain(inner_ty))
            }
            Exp::Sleep(duration) => {
                if duration.validate_async_syntax(ctx)?.is_promise() {
                    return Err("sleep duration cannot be a Promise value".to_string());
                }
                Ok(AsyncExprKind::Promise(BType::Void))
            }
            Exp::PromiseWait(promise) => {
                reject_unsupported_await_position(promise, "Promise.wait receiver")?;
                let kind = promise.validate_async_syntax(ctx)?;
                match kind {
                    AsyncExprKind::Promise(_inner) => {
                        ctx.wait_promise_exp(promise);
                        Ok(AsyncExprKind::Plain(_inner))
                    }
                    AsyncExprKind::Plain(_) => {
                        Err("Promise.wait() expects a Promise value".to_string())
                    }
                }
            }
            Exp::UnaryExp(_, exp) => {
                if exp.validate_async_syntax(ctx)?.is_promise() {
                    return Err(
                        "Promise value cannot be used in unary expression without await"
                            .to_string(),
                    );
                }
                Ok(AsyncExprKind::Plain(BType::I32))
            }
            Exp::BinaryExp(_, lhs, rhs) | Exp::ArrGet(lhs, rhs) => {
                if lhs.validate_async_syntax(ctx)?.is_promise()
                    || rhs.validate_async_syntax(ctx)?.is_promise()
                {
                    return Err(
                        "Promise value cannot be used in expression without await".to_string()
                    );
                }
                Ok(AsyncExprKind::Plain(BType::I32))
            }
            Exp::FuncCall(_, args) => {
                for arg in args {
                    if arg.validate_async_syntax(ctx)?.is_promise() {
                        return Err(
                            "Promise value cannot be passed as a normal argument without await"
                                .to_string(),
                        );
                    }
                }
                if let Exp::FuncCall(name, _) = self {
                    if let Some(sig) = ctx.funcs.get(name) {
                        if sig.is_async {
                            return Ok(AsyncExprKind::Promise(sig.ret.clone()));
                        }
                    }
                }
                Ok(AsyncExprKind::Plain(BType::I32))
            }
            Exp::Field(base, _) | Exp::PtrField(base, _) => {
                if base.validate_async_syntax(ctx)?.is_promise() {
                    return Err(
                        "Promise value cannot be used for field access without await".to_string(),
                    );
                }
                Ok(AsyncExprKind::Plain(BType::I32))
            }
            Exp::Ident(name) => Ok(ctx
                .lookup_promise(name)
                .map_or(AsyncExprKind::Plain(BType::I32), AsyncExprKind::Promise)),
            Exp::Number(_) => Ok(AsyncExprKind::Plain(BType::I32)),
            Exp::New(ty) => Ok(AsyncExprKind::Plain(BType::Ptr(Box::new(ty.clone())))),
        }
    }

    fn validate_semantics(
        &self,
        ctx: &mut SemanticCtx<'_>,
        loop_depth: usize,
    ) -> Result<AsyncExprKind, String> {
        match self {
            Exp::Await(inner) => {
                if !ctx.in_async_fn {
                    return Err("await is only allowed inside async functions".to_string());
                }
                let kind = inner.validate_semantics(ctx, loop_depth)?;
                let AsyncExprKind::Promise(inner_ty) = kind else {
                    return Err("await expects a Promise value".to_string());
                };
                Ok(AsyncExprKind::Plain(inner_ty))
            }
            Exp::FuncCall(name, args) => {
                let sig = ctx
                    .program
                    .funcs
                    .get(name)
                    .ok_or_else(|| format!("Unknown function {}", name))?;
                if args.len() != sig.param_count {
                    return Err(format!(
                        "Function {} expects {} argument(s), got {}",
                        name,
                        sig.param_count,
                        args.len()
                    ));
                }
                for (idx, arg) in args.iter().enumerate() {
                    let kind = arg.validate_semantics(ctx, loop_depth)?;
                    if let (Some(expected), Some(actual)) = (sig.params.get(idx), kind.plain_ty()) {
                        if !call_arg_compatible(expected, actual) {
                            return Err(format!(
                                "Function {} argument {} type mismatch: expected {}, got {}",
                                name,
                                idx + 1,
                                btype_name(expected),
                                btype_name(actual)
                            ));
                        }
                    }
                }
                if sig.is_async {
                    Ok(AsyncExprKind::Promise(sig.ret.clone()))
                } else {
                    Ok(AsyncExprKind::Plain(sig.ret.clone()))
                }
            }
            Exp::Sleep(duration) => {
                duration.validate_semantics(ctx, loop_depth)?;
                Ok(AsyncExprKind::Promise(BType::Void))
            }
            Exp::PromiseWait(inner)
            | Exp::UnaryExp(_, inner)
            | Exp::Field(inner, _)
            | Exp::PtrField(inner, _) => {
                let inner_kind = inner.validate_semantics(ctx, loop_depth)?;
                match self {
                    Exp::Field(_, field) => field_access_type(ctx, inner_kind.plain_ty(), field),
                    Exp::PtrField(_, field) => {
                        ptr_field_access_type(ctx, inner_kind.plain_ty(), field)
                    }
                    Exp::UnaryExp(UnaryOp::Addr, _) => Ok(AsyncExprKind::Plain(BType::Ptr(
                        Box::new(inner_kind.plain_ty().cloned().unwrap_or(BType::I32)),
                    ))),
                    Exp::UnaryExp(UnaryOp::Deref, _) => match inner_kind.plain_ty() {
                        Some(BType::Ptr(inner)) => Ok(AsyncExprKind::Plain((**inner).clone())),
                        _ => Ok(AsyncExprKind::Plain(BType::I32)),
                    },
                    Exp::PromiseWait(_) => match inner_kind {
                        AsyncExprKind::Promise(inner) => Ok(AsyncExprKind::Plain(inner)),
                        AsyncExprKind::Plain(_) => {
                            Err("Promise.wait() expects a Promise value".to_string())
                        }
                    },
                    _ => Ok(AsyncExprKind::Plain(BType::I32)),
                }
            }
            Exp::BinaryExp(_, lhs, rhs) | Exp::ArrGet(lhs, rhs) => {
                let lhs_kind = lhs.validate_semantics(ctx, loop_depth)?;
                rhs.validate_semantics(ctx, loop_depth)?;
                match self {
                    Exp::ArrGet(_, _) => array_element_type(lhs_kind.plain_ty()),
                    _ => Ok(AsyncExprKind::Plain(BType::I32)),
                }
            }
            Exp::Number(_) => Ok(AsyncExprKind::Plain(BType::I32)),
            Exp::Ident(name) => match ctx.lookup_var(name).unwrap_or(BType::I32) {
                BType::Promise(inner) => Ok(AsyncExprKind::Promise(*inner)),
                ty => Ok(AsyncExprKind::Plain(ty)),
            },
            Exp::New(ty) => Ok(AsyncExprKind::Plain(BType::Ptr(Box::new(ty.clone())))),
        }
    }
}

#[derive(Debug, Clone)]
struct AsyncFunctionSig {
    is_async: bool,
    ret: BType,
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

struct AsyncValidationContext<'a> {
    funcs: &'a HashMap<String, AsyncFunctionSig>,
    in_async_fn: bool,
    scopes: Vec<HashMap<String, BType>>,
    declared_promises: HashSet<String>,
    consumed_promises: HashSet<String>,
}

struct SemanticCtx<'a> {
    program: &'a SemanticProgram,
    in_async_fn: bool,
    ret_ty: &'a BType,
    scopes: Vec<HashMap<String, BType>>,
}

impl<'a> SemanticCtx<'a> {
    fn new(program: &'a SemanticProgram, in_async_fn: bool, ret_ty: &'a BType) -> Self {
        Self {
            program,
            in_async_fn,
            ret_ty,
            scopes: Vec::new(),
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn define_var(&mut self, name: String, ty: BType) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name, ty);
        }
    }

    fn lookup_var(&self, name: &str) -> Option<BType> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }
}

impl<'a> AsyncValidationContext<'a> {
    fn new(funcs: &'a HashMap<String, AsyncFunctionSig>, in_async_fn: bool) -> Self {
        Self {
            funcs,
            in_async_fn,
            scopes: Vec::new(),
            declared_promises: HashSet::new(),
            consumed_promises: HashSet::new(),
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn define_promise(&mut self, name: String, ty: BType) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.clone(), ty);
        }
        self.declared_promises.insert(name);
    }

    fn lookup_promise(&self, name: &str) -> Option<BType> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    fn wait_promise_exp(&mut self, exp: &Exp) {
        if let Exp::Ident(name) = exp {
            self.consumed_promises.insert(name.clone());
        }
    }

    fn await_promise_exp(&mut self, exp: &Exp) {
        if let Exp::Ident(name) = exp {
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

fn type_base_btype(ty: &Type) -> &BType {
    match ty {
        Type::BType(btype) | Type::Const(btype) => btype,
    }
}

fn var_decl_name(var: &VarDecl) -> String {
    match var {
        VarDecl::Ident(name) => name.clone(),
        VarDecl::Array(inner, _) => var_decl_name(inner),
    }
}

fn decl_btype(ty: &Type, var: &VarDecl) -> BType {
    let mut base = type_base_btype(ty).clone();
    let mut dims = Vec::new();
    collect_decl_dims(var, &mut dims);
    for dim in dims.into_iter().rev() {
        base = BType::Array(dim, Box::new(base));
    }
    base
}

fn collect_decl_dims(var: &VarDecl, dims: &mut Vec<usize>) {
    match var {
        VarDecl::Ident(_) => {}
        VarDecl::Array(inner, len) => {
            collect_decl_dims(inner, dims);
            let len = eval_const_exp_with(len, &|_| None).unwrap_or(0).max(0) as usize;
            dims.push(len);
        }
    }
}

fn func_param_semantic_btype(param: &FuncParam) -> BType {
    if let Some(dims) = &param.array_dims {
        let dims = dims
            .iter()
            .map(|dim| eval_const_exp_with(dim, &|_| None).unwrap_or(0).max(0) as usize)
            .collect::<Vec<_>>();
        func_param_btype(param.btype.clone(), dims)
    } else {
        param.btype.clone()
    }
}

fn call_arg_compatible(expected: &BType, actual: &BType) -> bool {
    btype_same(expected, actual)
        || matches!((expected, actual), (BType::Ptr(inner), BType::Array(_, actual_inner)) if btype_same(inner, actual_inner))
}

fn array_element_type(ty: Option<&BType>) -> Result<AsyncExprKind, String> {
    match ty {
        Some(BType::Array(_, inner)) | Some(BType::Ptr(inner)) => {
            Ok(AsyncExprKind::Plain((**inner).clone()))
        }
        _ => Ok(AsyncExprKind::Plain(BType::I32)),
    }
}

fn field_access_type(
    ctx: &SemanticCtx<'_>,
    base: Option<&BType>,
    field: &str,
) -> Result<AsyncExprKind, String> {
    match base {
        Some(BType::Struct(name)) => {
            let ty = ctx
                .program
                .structs
                .get(name)
                .and_then(|sig| sig.fields.get(field))
                .cloned()
                .unwrap_or(BType::I32);
            Ok(AsyncExprKind::Plain(ty))
        }
        _ => Ok(AsyncExprKind::Plain(BType::I32)),
    }
}

fn ptr_field_access_type(
    ctx: &SemanticCtx<'_>,
    base: Option<&BType>,
    field: &str,
) -> Result<AsyncExprKind, String> {
    match base {
        Some(BType::Ptr(inner)) => field_access_type(ctx, Some(inner), field),
        _ => Ok(AsyncExprKind::Plain(BType::I32)),
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
    for (name, ret, param_count) in [
        ("getint", BType::I32, 0),
        ("getch", BType::I32, 0),
        ("getarray", BType::I32, 1),
        ("putint", BType::Void, 1),
        ("putch", BType::I32, 1),
        ("putarray", BType::Void, 2),
        ("starttime", BType::Void, 0),
        ("stoptime", BType::Void, 0),
    ] {
        funcs.insert(
            name.to_string(),
            FunctionSig {
                is_async: false,
                ret,
                param_count,
                params: vec![BType::I32; param_count],
            },
        );
    }
    funcs
}

fn reject_unsupported_await_nested_in_init(init: &InitVal, context: &str) -> Result<(), String> {
    match init {
        InitVal::Exp(exp) => reject_unsupported_await_nested(exp, context),
        InitVal::Arr(items) => {
            for item in items {
                if init_contains_await(item) {
                    return Err(format!(
                        "await inside {} is not supported by callback-hole lowering yet",
                        context
                    ));
                }
            }
            Ok(())
        }
    }
}

fn reject_unsupported_await_nested(exp: &Exp, context: &str) -> Result<(), String> {
    match exp {
        Exp::Await(_) => Ok(()),
        _ => reject_unsupported_await_position(exp, context),
    }
}

fn reject_unsupported_await_position(exp: &Exp, context: &str) -> Result<(), String> {
    if exp_contains_await(exp) {
        Err(format!(
            "await inside {} is not supported by callback-hole lowering yet",
            context
        ))
    } else {
        Ok(())
    }
}

fn init_contains_await(init: &InitVal) -> bool {
    match init {
        InitVal::Exp(exp) => exp_contains_await(exp),
        InitVal::Arr(items) => items.iter().any(init_contains_await),
    }
}

fn exp_contains_await(exp: &Exp) -> bool {
    match exp {
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
