use std::fmt::Debug;

pub trait AstNode: Debug {
    fn to_koopa_ir(&self) -> String;
}

#[derive(Debug)]
pub struct CompUnit {
    pub func_def: FuncDef,
}

impl AstNode for CompUnit {
    fn to_koopa_ir(&self) -> String {
        self.func_def.to_koopa_ir()
    }
}

#[derive(Debug)]
pub struct FuncDef {
    pub func_type: FuncType,
    pub ident: String,
    pub block: Block,
}

impl AstNode for FuncDef {
    fn to_koopa_ir(&self) -> String {
        let mut ir = String::new();
        let func_type_ir = self.func_type.to_koopa_ir();
        ir.push_str(&format!("fun @{}(): {} {{\n%entry:\n", self.ident, func_type_ir));
        ir.push_str(&self.block.to_koopa_ir());
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
    fn to_koopa_ir(&self) -> String {
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
    fn to_koopa_ir(&self) -> String {
        let mut ir = String::new();
        for stmt in &self.stmts {
            ir.push_str(&stmt.to_koopa_ir());
        }
        ir
    }
}

#[derive(Debug)]
pub enum Stmt {
    Return(i32),
}

impl AstNode for Stmt {
    fn to_koopa_ir(&self) -> String {
        match self {
            Stmt::Return(num) => format!("  ret {}\n", num),
        }
    }
}
