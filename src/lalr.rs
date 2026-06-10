#[derive(Debug, Clone)]
pub struct CompUnit {
    pub glob_defs: Vec<GlobleDef>,
}

#[derive(Debug, Clone)]
pub enum GlobleDef {
    FuncDef(FuncDef),
    GlobleDecl(Type, Vec<SingleDecl>), // @global = decl type, init
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
    AddSyncFunc(Exp),
    Wait,
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
    FuncCall(String, Vec<Exp>),
    ArrGet(Box<Exp>, Box<Exp>),
    Field(Box<Exp>, String),
    PtrField(Box<Exp>, String),
    Ident(String),
}

impl CompUnit {
    pub fn contains_async_syntax(&self) -> bool {
        self.glob_defs.iter().any(GlobleDef::contains_async_syntax)
    }

    pub fn validate_async_syntax(&self) -> Result<(), String> {
        for glob_def in &self.glob_defs {
            glob_def.validate_async_syntax()?;
        }
        Ok(())
    }
}

impl GlobleDef {
    fn contains_async_syntax(&self) -> bool {
        match self {
            GlobleDef::FuncDef(func) => func.contains_async_syntax(),
            GlobleDef::GlobleDecl(ty, decls) => {
                type_contains_promise(ty)
                    || decls.iter().any(|decl| {
                        decl.init
                            .as_ref()
                            .is_some_and(InitVal::contains_async_syntax)
                    })
            }
            GlobleDef::StructDef(def) => def
                .fields
                .iter()
                .any(|field| type_contains_promise(&field.ty)),
        }
    }

    fn validate_async_syntax(&self) -> Result<(), String> {
        match self {
            GlobleDef::FuncDef(func) => func.validate_async_syntax(),
            GlobleDef::GlobleDecl(_, decls) => {
                for decl in decls {
                    if let Some(init) = &decl.init {
                        init.validate_async_syntax(false)?;
                    }
                }
                Ok(())
            }
            GlobleDef::StructDef(_) => Ok(()),
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

    fn validate_async_syntax(&self) -> Result<(), String> {
        for stmt in &self.block {
            stmt.validate_async_syntax(self.is_async)?;
        }
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
            Stmt::Exp(exp) | Stmt::Return(Some(exp)) | Stmt::AddSyncFunc(exp) => {
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
            Stmt::Wait => true,
            Stmt::Continue | Stmt::Break | Stmt::Return(None) | Stmt::Empty => false,
        }
    }

    fn validate_async_syntax(&self, in_async_fn: bool) -> Result<(), String> {
        match self {
            Stmt::Block(stmts) => {
                for stmt in stmts {
                    stmt.validate_async_syntax(in_async_fn)?;
                }
                Ok(())
            }
            Stmt::Assign(lhs, rhs) => {
                lhs.validate_async_syntax(in_async_fn)?;
                rhs.validate_async_syntax(in_async_fn)
            }
            Stmt::Decl(_, decls) => {
                for decl in decls {
                    if let Some(init) = &decl.init {
                        init.validate_async_syntax(in_async_fn)?;
                    }
                }
                Ok(())
            }
            Stmt::Exp(exp) | Stmt::Return(Some(exp)) | Stmt::AddSyncFunc(exp) => {
                exp.validate_async_syntax(in_async_fn)
            }
            Stmt::If(cond, then_stmt) => {
                cond.validate_async_syntax(in_async_fn)?;
                then_stmt.validate_async_syntax(in_async_fn)
            }
            Stmt::IfElse(cond, then_stmt, else_stmt) => {
                cond.validate_async_syntax(in_async_fn)?;
                then_stmt.validate_async_syntax(in_async_fn)?;
                else_stmt.validate_async_syntax(in_async_fn)
            }
            Stmt::While(cond, body) => {
                cond.validate_async_syntax(in_async_fn)?;
                body.validate_async_syntax(in_async_fn)
            }
            Stmt::Wait | Stmt::Continue | Stmt::Break | Stmt::Return(None) | Stmt::Empty => Ok(()),
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

    fn validate_async_syntax(&self, in_async_fn: bool) -> Result<(), String> {
        match self {
            InitVal::Exp(exp) => exp.validate_async_syntax(in_async_fn),
            InitVal::Arr(items) => {
                for item in items {
                    item.validate_async_syntax(in_async_fn)?;
                }
                Ok(())
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
            Exp::Field(base, _) | Exp::PtrField(base, _) => base.contains_async_syntax(),
            Exp::New(ty) => btype_contains_promise(ty),
            Exp::Number(_) | Exp::Ident(_) => false,
        }
    }

    fn validate_async_syntax(&self, in_async_fn: bool) -> Result<(), String> {
        match self {
            Exp::Await(inner) => {
                if !in_async_fn {
                    return Err("await is only allowed inside async functions".to_string());
                }
                inner.validate_async_syntax(in_async_fn)
            }
            Exp::Sleep(duration) => duration.validate_async_syntax(in_async_fn),
            Exp::UnaryExp(_, exp) => exp.validate_async_syntax(in_async_fn),
            Exp::BinaryExp(_, lhs, rhs) | Exp::ArrGet(lhs, rhs) => {
                lhs.validate_async_syntax(in_async_fn)?;
                rhs.validate_async_syntax(in_async_fn)
            }
            Exp::FuncCall(_, args) => {
                for arg in args {
                    arg.validate_async_syntax(in_async_fn)?;
                }
                Ok(())
            }
            Exp::Field(base, _) | Exp::PtrField(base, _) => base.validate_async_syntax(in_async_fn),
            Exp::Number(_) | Exp::New(_) | Exp::Ident(_) => Ok(()),
        }
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
