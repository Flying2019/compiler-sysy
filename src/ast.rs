use std::{collections::HashMap, fmt::Debug};

use koopa::ir::values::Integer;

use crate::ast_tool::{eval_binary_const, eval_unary_const, gen_binary_koopa_ir, gen_unary_koopa_ir};

pub struct Background {
    temp_counter: usize,
    variable_map: HashMap<String, String>,
    constant_map: HashMap<String, i32>,
}

impl Background {
    pub fn new() -> Self {
        Self {
            temp_counter: 0,
            variable_map: HashMap::new(),
            constant_map: HashMap::new(),
        }
    }

    pub fn next_temp(&mut self) -> String {
        let temp_name = format!("%{}", self.temp_counter);
        self.temp_counter += 1;
        temp_name
    }

    pub fn get_variable(&mut self, name: String) -> String {
        if let Some(index) = self.variable_map.get(&name) {
            format!("%{}_{}", name, index)
        } else {
            panic!("Variable {} not found in background", name);
        }
    }

    pub fn set_variable(&mut self, name: String, value: String) {
        self.variable_map.insert(name, value);
    }
}

#[derive(Debug, Clone)]
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
    pub fn extend(&self, additional: ReturnValue) -> Self {
        let new_value = additional.value;
        let new_content = format!("{}{}", self.content, additional.content);
        Self { value: new_value, content: new_content }
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
    Assign(String, Exp),
    Decl(Type, Vec<SingleDecl>),
    Return(Exp),
}

impl AstNode for Stmt {
    fn to_koopa_ir(&self, background: &mut Background) -> String {
        match self {
            Stmt::Assign(ident, exp) => {
                let exp_ret = exp.to_koopa(background);
                background.set_variable(ident.clone(), exp_ret.value.unwrap());
                exp_ret.content
            }
            Stmt::Decl(typ, decls) => {
                let mut content = String::new();
                match typ {
                    Type::Var(_t) => {
                        for decl in decls {
                            if let Some(init) = &decl.init {
                                let init_ret = init.to_koopa(background);
                                content.push_str(&init_ret.content);
                                background.set_variable(decl.ident.clone(), init_ret.value.unwrap());
                            } else {
                                let var_name = background.next_temp();
                                background.set_variable(decl.ident.clone(), var_name);
                            }
                        }
                        content
                    }
                    Type::Const(_t) => {
                        for decl in decls {
                            if let Some(init) = &decl.init {
                                let value = init.try_eval_const().expect("Const variable must be initialized with a constant expression");
                                background.constant_map.insert(decl.ident.clone(), value);
                            } else {
                                panic!("Const variable {} must be initialized", decl.ident);
                            }
                        }
                        content
                    }
                }
            }
            Stmt::Return(num) => {
                let ret = num.to_koopa(background);
                let additional_ir = format!("  ret {}\n", ret.value.unwrap());
                format!("{}{}", ret.content, additional_ir)
            }
        }
    }
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

impl AstNode for Exp {
    fn to_koopa(&self, background: &mut Background) -> ReturnValue {
        match self {
            Exp::Number(num) => ReturnValue::new(Some(num.to_string()), String::new()),
            Exp::UnaryExp(op, exp) => {
                let exp_ret = exp.to_koopa(background);
                let src = exp_ret.value.clone().unwrap();
                exp_ret.extend(
                    gen_unary_koopa_ir(op, src, background)
                )
            }
            Exp::BinaryExp(op, left, right) => {
                let left_ret = left.to_koopa(background);
                let right_ret = right.to_koopa(background);
                let src_1 = left_ret.value.clone().unwrap();
                let src_2 = right_ret.value.clone().unwrap();
                left_ret.extend(right_ret).extend(
                    gen_binary_koopa_ir(op, src_1, src_2, background)
                )
            }
            Exp::Ident(name) => {
                if let Some(const_value) = background.constant_map.get(name) {
                    ReturnValue::new(Some(const_value.to_string()), String::new())
                } else {
                    let var_name = background.get_variable(name.clone());
                    ReturnValue::new(Some(var_name), String::new())
                }
            }
        }
    }
}

impl Exp {
    fn try_eval_const(&self) -> Option<i32> {
        match self {
            Exp::Number(num) => Some(*num),
            Exp::UnaryExp(op, exp) => {
                let value = exp.try_eval_const()?;
                Some(eval_unary_const(op, value))
            }
            Exp::BinaryExp(op, left, right) => {
                let lhs = left.try_eval_const()?;
                let rhs = right.try_eval_const()?;
                Some(eval_binary_const(op, lhs, rhs))
            }
            Exp::Ident(_) => None,
        }
    }
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