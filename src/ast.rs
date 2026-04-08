use std::{collections::HashMap, fmt::Debug};
use crate::ast_tool::{eval_binary_const, eval_unary_const, gen_binary_koopa_ir, gen_unary_koopa_ir};
use crate::lalr::*;

#[derive(Clone)]
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
        self.variable_map.get(&name).cloned().unwrap_or_else(|| {
            panic!("Variable {} not found in background", name);
        })
    }

    pub fn set_variable(&mut self, name: String, value: String) {
        self.variable_map.insert(name, value);
    }

    pub fn try_get_constant(&self, name: &String) -> Option<i32> {
        self.constant_map.get(name).cloned()
    }

    pub fn set_constant(&mut self, name: String, value: i32) {
        self.constant_map.insert(name, value);
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
    fn to_koopa(&self, bg: &mut Background) -> ReturnValue {
        ReturnValue::with_content(String::new())
    }
    fn to_koopa_ir(&self, bg: &mut Background) -> String {
        self.to_koopa(bg).content
    }
}

impl AstNode for CompUnit {
    fn to_koopa_ir(&self, bg: &mut Background) -> String {
        self.func_def.to_koopa_ir(bg)
    }
}

impl AstNode for FuncDef {
    fn to_koopa_ir(&self, bg: &mut Background) -> String {
        let mut ir = String::new();
        let func_type_ir = self.func_type.to_koopa_ir(bg);
        let block_ir = self.block.to_koopa_ir(bg);
        ir.push_str(&format!("fun @{}(): {} {{\n%entry:\n", self.ident, func_type_ir));
        ir.push_str(&block_ir);
        ir.push_str("}\n");
        ir
    }
}

impl AstNode for FuncType {
    fn to_koopa_ir(&self, _bg: &mut Background) -> String {
        match self {
            FuncType::Void => "void".to_string(),
            FuncType::Int => "i32".to_string(),
        }
    }
}

impl AstNode for Block {
    fn to_koopa_ir(&self, bg: &mut Background) -> String {
        let mut ir = String::new();
        for stmt in &self.stmts {
            ir.push_str(&stmt.to_koopa_ir(bg));
        }
        ir
    }
}

impl AstNode for Stmt {
    fn to_koopa_ir(&self, bg: &mut Background) -> String {
        match self {
            Stmt::Block(block) => block.to_koopa_ir(bg),
            Stmt::Assign(ident, exp) => {
                let exp_ret = exp.to_koopa(bg);
                bg.set_variable(ident.clone(), exp_ret.value.unwrap());
                exp_ret.content
            }
            Stmt::Decl(typ, decls) => {
                let mut content = String::new();
                match typ {
                    Type::Var(_t) => {
                        for decl in decls {
                            if let Some(init) = &decl.init {
                                let init_ret = init.to_koopa(bg);
                                content.push_str(&init_ret.content);
                                bg.set_variable(decl.ident.clone(), init_ret.value.unwrap());
                            } else {
                                let var_name = bg.next_temp();
                                bg.set_variable(decl.ident.clone(), var_name);
                            }
                        }
                        content
                    }
                    Type::Const(_t) => {
                        for decl in decls {
                            if let Some(init) = &decl.init {
                                let value = init.try_eval_const(bg).expect("Const variable must be initialized with a constant expression");
                                bg.set_constant(decl.ident.clone(), value);
                            } else {
                                panic!("Const variable {} must be initialized", decl.ident);
                            }
                        }
                        content
                    }
                }
            }
            Stmt::Return(num) => {
                let ret = num.to_koopa(bg);
                let additional_ir = format!("  ret {}\n", ret.value.unwrap());
                format!("{}{}", ret.content, additional_ir)
            }
        }
    }
}

impl AstNode for Exp {
    fn to_koopa(&self, bg: &mut Background) -> ReturnValue {
        match self {
            Exp::Number(num) => ReturnValue::new(Some(num.to_string()), String::new()),
            Exp::UnaryExp(op, exp) => {
                let exp_ret = exp.to_koopa(bg);
                let src = exp_ret.value.clone().unwrap();
                exp_ret.extend(
                    gen_unary_koopa_ir(op, src, bg)
                )
            }
            Exp::BinaryExp(op, left, right) => {
                let left_ret = left.to_koopa(bg);
                let right_ret = right.to_koopa(bg);
                let src_1 = left_ret.value.clone().unwrap();
                let src_2 = right_ret.value.clone().unwrap();
                left_ret.extend(right_ret).extend(
                    gen_binary_koopa_ir(op, src_1, src_2, bg)
                )
            }
            Exp::Ident(name) => {
                if let Some(const_value) = bg.try_get_constant(name) {
                    ReturnValue::new(Some(const_value.to_string()), String::new())
                } else {
                    let var_name = bg.get_variable(name.clone());
                    ReturnValue::new(Some(var_name), String::new())
                }
            }
        }
    }
}

impl Exp {
    fn try_eval_const(&self, bg: &Background) -> Option<i32> {
        match self {
            Exp::Number(num) => Some(*num),
            Exp::UnaryExp(op, exp) => {
                let value = exp.try_eval_const(bg)?;
                Some(eval_unary_const(op, value))
            }
            Exp::BinaryExp(op, left, right) => {
                let lhs = left.try_eval_const(bg)?;
                let rhs = right.try_eval_const(bg)?;
                Some(eval_binary_const(op, lhs, rhs))
            }
            Exp::Ident(name) => {
                bg.try_get_constant(name)
            }
        }
    }
}
