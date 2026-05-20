#[derive(Debug, Clone)]
pub struct CompUnit {
    pub glob_defs: Vec<GlobleDef>,
}

#[derive(Debug, Clone)]
pub enum GlobleDef {
    FuncDef(FuncDef),
    GlobleDecl(Type, Vec<SingleDecl>), // @global = decl type, init
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
    Ptr(Box<BType>),
}

#[derive(Debug, Clone)]
pub enum Type {
    BType(BType),
    Const(BType),
}

#[derive(Debug, Clone)]
pub enum Exp {
    Number(i32),
    UnaryExp(UnaryOp, Box<Exp>),
    BinaryExp(BinaryOp, Box<Exp>, Box<Exp>),
    FuncCall(String, Vec<Exp>),
    ArrGet(Box<Exp>, Box<Exp>),
    Ident(String),
}

#[derive(Debug, Clone)]
pub enum UnaryOp {
    Pos,
    Neg,
    Not,
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
