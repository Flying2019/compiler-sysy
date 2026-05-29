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
    Continue,
    Break,
    Return(Option<Exp>),
    Empty,
}

#[derive(Debug, Clone)]
pub enum VarDecl{
    Ident(String),
    Array(Box<VarDecl>, Exp)
}

#[derive(Debug, Clone)]
pub struct SingleDecl {
    pub var: VarDecl,
    pub init: Option<InitVal>,
}

#[derive(Debug, Clone)]
pub enum InitVal {
    Exp(Exp),
    Arr(Vec<InitVal>)
}

#[derive(Debug, Clone)]
pub enum BType {
    I32,
    Void,
    Struct(String),
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
        Exp::New(_) | Exp::FuncCall(_, _) | Exp::ArrGet(_, _) | Exp::Field(_, _) | Exp::PtrField(_, _) => None,
    }
}

pub fn eval_param_dim<F>(exp: &Exp, lookup: &F) -> usize
where
    F: Fn(&str) -> Option<i32>,
{
    let value = eval_const_exp_with(exp, lookup)
        .expect("Array parameter dimensions must be constant expressions");
    assert!(value >= 0, "Array parameter dimensions must be non-negative");
    value as usize
}

#[derive(Debug, Clone)]
pub enum Exp {
    Number(i32),
    UnaryExp(UnaryOp, Box<Exp>),
    BinaryExp(BinaryOp, Box<Exp>, Box<Exp>),
    New(BType),
    FuncCall(String, Vec<Exp>),
    ArrGet(Box<Exp>, Box<Exp>),
    Field(Box<Exp>, String),
    PtrField(Box<Exp>, String),
    Ident(String),
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
