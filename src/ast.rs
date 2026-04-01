use std::fmt::Debug;

pub struct Background {
    temp_counter: usize,
}

impl Background {
    pub fn new() -> Self {
        Self { temp_counter: 0 }
    }

    pub fn next_temp(&mut self) -> String {
        let temp_name = format!("%{}", self.temp_counter);
        self.temp_counter += 1;
        temp_name
    }

    pub fn last_temp(&self) -> String {
        if self.temp_counter == 0 {
            panic!("No temporary variables have been generated yet.");
        }
        format!("%{}", self.temp_counter - 1)
    }
}

pub struct ReturnValue {
    pub value: Option<String>,
    pub content: String,
}

impl ReturnValue {
    pub fn new(value: Option<String>, content: String) -> Self {
        Self { value, content }
    }
    pub fn with_content(content: String) -> Self {
        Self { value: None, content }
    }
}

#[allow(unused)]
pub trait AstNode: Debug {
    fn to_koopa(&self, background: &mut Background) -> ReturnValue {
        ReturnValue::with_content(String::new())
    }
    fn to_koopa_ir(&self, background: &mut Background) -> String {
        self.to_koopa(background).content
    }
}

#[derive(Debug)]
pub struct CompUnit {
    pub func_def: FuncDef,
}

impl AstNode for CompUnit {
    fn to_koopa_ir(&self, background: &mut Background) -> String {
        self.func_def.to_koopa_ir(background)
    }
}

#[derive(Debug)]
pub struct FuncDef {
    pub func_type: FuncType,
    pub ident: String,
    pub block: Block,
}

impl AstNode for FuncDef {
    fn to_koopa_ir(&self, background: &mut Background) -> String {
        let mut ir = String::new();
        let func_type_ir = self.func_type.to_koopa_ir(background);
        let block_ir = self.block.to_koopa_ir(background);
        ir.push_str(&format!("fun @{}(): {} {{\n%entry:\n", self.ident, func_type_ir));
        ir.push_str(&block_ir);
        ir.push_str("}\n");
        ir
    }
}

#[derive(Debug)]
pub enum FuncType {
    Void,
    Int,
}

impl AstNode for FuncType {
    fn to_koopa_ir(&self, _background: &mut Background) -> String {
        match self {
            FuncType::Void => "void".to_string(),
            FuncType::Int => "i32".to_string(),
        }
    }
}

#[derive(Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
}

impl AstNode for Block {
    fn to_koopa_ir(&self, background: &mut Background) -> String {
        let mut ir = String::new();
        for stmt in &self.stmts {
            ir.push_str(&stmt.to_koopa_ir(background));
        }
        ir
    }
}

#[derive(Debug)]
pub enum Stmt {
    Return(Exp),
}

impl AstNode for Stmt {
    fn to_koopa_ir(&self, background: &mut Background) -> String {
        match self {
            Stmt::Return(num) => {
                let ret = num.to_koopa(background);
                let additional_ir = format!("  ret {}\n", ret.value.unwrap());
                format!("{}{}", ret.content, additional_ir)
            }
        }
    }
}

#[derive(Debug)]
pub enum Exp {
    Number(i32),
    UnaryExp(UnaryOp, Box<Exp>),
    BinaryExp(BinaryOp, Box<Exp>, Box<Exp>),
}

impl AstNode for Exp {
    fn to_koopa(&self, background: &mut Background) -> ReturnValue {
        match self {
            Exp::Number(num) => ReturnValue::new(Some(num.to_string()), String::new()),
            Exp::UnaryExp(op, exp) => {
                let exp_ret = exp.to_koopa(background);
                let temp = background.next_temp();
                let additional_ir = format!("  {} = {} 0, {}\n", temp, op.to_koopa_ir(background), exp_ret.value.unwrap());
                ReturnValue::new(Some(temp), format!("{}{}", exp_ret.content, additional_ir))
            }
            Exp::BinaryExp(op, left, right) => {
                let left_ret = left.to_koopa(background);
                let right_ret = right.to_koopa(background);
                let temp = background.next_temp();
                let additional_ir = format!("  {} = {} {}, {}\n", temp, op.to_koopa_ir(background), left_ret.value.unwrap(), right_ret.value.unwrap());
                ReturnValue::new(Some(temp), format!("{}{}{}", left_ret.content, right_ret.content, additional_ir))
            }
        }
    }
}

#[derive(Debug)]
pub enum UnaryOp {
    Pos,
    Neg,
    Not,
}

impl AstNode for UnaryOp {
    fn to_koopa_ir(&self, _background: &mut Background) -> String {
        match self {
            UnaryOp::Pos => "add".to_string(),
            UnaryOp::Neg => "sub".to_string(),
            UnaryOp::Not => "eq".to_string(),
        }
    }
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

impl AstNode for BinaryOp {
    fn to_koopa_ir(&self, _background: &mut Background) -> String {
        match self {
            BinaryOp::Add => "add".to_string(),
            BinaryOp::Sub => "sub".to_string(),
            BinaryOp::Mul => "mul".to_string(),
            BinaryOp::Div => "div".to_string(),
            BinaryOp::Mod => "mod".to_string(),
            BinaryOp::Lt => "lt".to_string(),
            BinaryOp::Gt => "gt".to_string(),
            BinaryOp::Le => "le".to_string(),
            BinaryOp::Ge => "ge".to_string(),
            BinaryOp::Eq => "eq".to_string(),
            BinaryOp::Ne => "ne".to_string(),
            BinaryOp::And => "and".to_string(),
            BinaryOp::Or => "or".to_string(),
        }
    }
}