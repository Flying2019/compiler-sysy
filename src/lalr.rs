#[derive(Debug)]
pub struct CompUnit {
    pub func_def: FuncDef,
}

#[derive(Debug)]
pub struct FuncDef {
    pub func_type: FuncType,
    pub ident: String,
    pub block: Block,
}

#[derive(Debug)]
pub enum FuncType {
    Void,
    Int,
}

#[derive(Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
}

#[derive(Debug)]
pub enum Stmt {
    Block(Block),
    Assign(String, Exp),
    Decl(Type, Vec<SingleDecl>),
    Return(Exp),
    Exp(Exp),
    Empty,
}

#[derive(Debug)]
pub struct SingleDecl {
    pub ident: String,
    pub init: Option<Exp>,
}

#[derive(Debug)]
pub enum BType {
    Int,
}

#[derive(Debug)]
pub enum Type {
    Var(BType),
    Const(BType),
}

#[derive(Debug)]
pub enum Exp {
    Number(i32),
    UnaryExp(UnaryOp, Box<Exp>),
    BinaryExp(BinaryOp, Box<Exp>, Box<Exp>),
    Ident(String),
}

#[derive(Debug)]
pub enum UnaryOp {
    Pos,
    Neg,
    Not,
}

#[derive(Debug)]
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