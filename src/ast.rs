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
        }
    }
}

#[derive(Debug)]
pub enum UnaryOp {
    Pos,
    Neg,
    Not,
}

fn gen_unary_koopa_ir(op: &UnaryOp, src: String, background: &mut Background) -> ReturnValue {
    let dst = background.next_temp();
    let ir =
    match op {
        UnaryOp::Pos => format!("  {} = add 0, {}\n", dst, src),
        UnaryOp::Neg => format!("  {} = sub 0, {}\n", dst, src),
        UnaryOp::Not => format!("  {} = eq {}, 0\n", dst, src),
    };
    ReturnValue { value: Some(dst), content: ir }
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

fn gen_binary_koopa_ir(op: &BinaryOp, lhs: String, rhs: String, background: &mut Background) -> ReturnValue {
    match op {
        BinaryOp::And => {
            let dst_1 = background.next_temp();
            let dst_2 = background.next_temp();
            let dst = background.next_temp();
            let ir = format!(
                "  {} = ne {}, 0\n  {} = ne {}, 0\n  {} = and {}, {}\n",
                dst_1, lhs, dst_2, rhs, dst, dst_1, dst_2);
            return ReturnValue { value: Some(dst), content: ir }
        }
        BinaryOp::Or => {
            let dst_1 = background.next_temp();
            let dst = background.next_temp();
            let ir = format!(
                "  {} = or {}, {}\n  {} = ne {}, 0\n",
                dst_1, lhs, rhs, dst, dst_1);
            return ReturnValue { value: Some(dst), content: ir }
        }
        _ => {}
    };
    let dst = background.next_temp();
    let ir =
    match op {
        BinaryOp::Add => format!("  {} = add {}, {}\n", dst, lhs, rhs),
        BinaryOp::Sub => format!("  {} = sub {}, {}\n", dst, lhs, rhs),
        BinaryOp::Mul => format!("  {} = mul {}, {}\n", dst, lhs, rhs),
        BinaryOp::Div => format!("  {} = div {}, {}\n", dst, lhs, rhs),
        BinaryOp::Mod => format!("  {} = mod {}, {}\n", dst, lhs, rhs),
        BinaryOp::Lt => format!("  {} = lt {}, {}\n", dst, lhs, rhs),
        BinaryOp::Gt => format!("  {} = gt {}, {}\n", dst, lhs, rhs),
        BinaryOp::Le => format!("  {} = le {}, {}\n", dst, lhs, rhs),
        BinaryOp::Ge => format!("  {} = ge {}, {}\n", dst, lhs, rhs),
        BinaryOp::Eq => format!("  {} = eq {}, {}\n", dst, lhs, rhs),
        BinaryOp::Ne => format!("  {} = ne {}, {}\n", dst, lhs, rhs),
        _ => unimplemented!("Unsupported binary operation: {:?}", op),
    };
    ReturnValue { value: Some(dst), content: ir }
}