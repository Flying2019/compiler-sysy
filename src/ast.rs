use std::{collections::HashMap, fmt::Debug};
use crate::ast_tool::{eval_binary_const, eval_unary_const, gen_binary_koopa_ir, gen_unary_koopa_ir};
use crate::lalr::*;

pub struct RenameManager {
    timestamp: usize,
    rename_count: HashMap<String, usize>,
    rename_record: HashMap<String, Vec<(usize, usize)>>, // (timestamp, count)
}

fn variable_rename(name: String, count: usize) -> String {
    if count == 1 {
        format!("@{}", name)
    } else {
        format!("@{}_{}", name, count)
    }
}

impl RenameManager {
    pub fn new() -> Self {
        Self {
            timestamp: 0,
            rename_count: HashMap::new(),
            rename_record: HashMap::new(),
        }
    }
    
    pub fn new_variable(&mut self, name: String) -> Option<usize> {
        let count = self.rename_count.entry(name.clone()).or_insert(0);
        *count += 1;
        let times = self.rename_record.entry(name).or_insert(Vec::new());
        if let Some(last) = times.last() {
            if last.0 == self.timestamp {
                return None;
            }
        }
        times.push((self.timestamp, *count));
        Some(*count)
    }

    pub fn get_current_name(&self, name: &String) -> Option<usize> {
        let &(ts, count) = self.rename_record.get(name)?.last()?;
        if ts <= self.timestamp {
            Some(count)
        } else {
            // The variable is not visible
            None
        }
    }

    pub fn rollback(&mut self, timestamp: usize) {
        for (_, times) in self.rename_record.iter_mut() {
            while let Some(&(ts, _)) = times.last() {
                if ts > timestamp {
                    times.pop();
                } else {
                    break;
                }
            }
        }
    }

    pub fn increment(&mut self) {
        self.timestamp += 1;
    }

    pub fn record(&self) -> usize {
        self.timestamp
    }
}

pub struct Background {
    temp_counter: usize,
    branch_counter: usize,
    constant_map: HashMap<String, i32>,
    rename_manager: RenameManager,
    next_bb: Option<String>,
}

impl Background {
    pub fn new() -> Self {
        Self {
            temp_counter: 0,
            branch_counter: 0,
            constant_map: HashMap::new(),
            rename_manager: RenameManager::new(),
            next_bb: None,
        }
    }

    pub fn next_temp(&mut self) -> String {
        let temp_name = format!("%{}", self.temp_counter);
        self.rename_manager.new_variable(temp_name.clone()).expect("Temp variable name conflict");
        self.temp_counter += 1;
        temp_name
    }

    pub fn get_variable(&self, name: String) -> String {
        // println!("Getting variable: {}", name);
        let count = self.rename_manager.get_current_name(&name).expect("Variable not found in the current scope");
        variable_rename(name, count)
    }

    pub fn new_variable(&mut self, name: String) -> String {
        // println!("Creating variable: {}", name);
        let count = self.rename_manager.new_variable(name.clone()).expect("Variable name conflict in the same scope");
        let new_name = variable_rename(name.clone(), count);
        new_name
    }

    pub fn try_get_constant(&self, name: String) -> Option<i32> {
        let name = self.get_variable(name);
        self.constant_map.get(&name).cloned()
    }

    pub fn set_constant(&mut self, name: String, value: i32) {
        let name = self.new_variable(name);
        self.constant_map.insert(name, value);
    }

    pub fn increment_time(&mut self) {
        self.rename_manager.increment();
    }

    pub fn record(&self) -> usize {
        self.rename_manager.record()
    }

    pub fn rollback(&mut self, timestamp: usize) {
        self.rename_manager.rollback(timestamp);
    }

    pub fn new_branch(&mut self) -> String {
        let branch_name = format!("br{}", self.branch_counter);
        self.branch_counter += 1;
        branch_name
    }

    pub fn set_next_br(&mut self, branch: String) {
        self.next_bb = Some(branch);
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
        let block_ir = self.block.iter().map(|stmt| stmt.to_koopa_ir(bg)).collect::<String>();
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

impl AstNode for Stmt {
    fn to_koopa_ir(&self, bg: &mut Background) -> String {
        let next_bb: Option<String> = bg.next_bb.clone();
        match self {
            Stmt::Block(block) => {
                let ts = bg.record();
                bg.increment_time();
                let mut block_ir = String::new();
                // Only the last statement in the block set bb to next_bb. Others set to None.
                for (id, stmt) in block.iter().enumerate() {
                    if id == block.len() - 1 && next_bb.is_some() {
                        bg.next_bb = next_bb.clone();
                    } else {
                        bg.next_bb = None;
                    }
                    block_ir.push_str(&stmt.to_koopa_ir(bg));
                }
                bg.rollback(ts);
                block_ir
            }
            Stmt::Assign(ident, exp) => {
                let exp_ret = exp.to_koopa(bg);
                let ptr = bg.get_variable(ident.clone());
                let store_ir = format!("\tstore {}, {}\n", exp_ret.value.unwrap(), ptr);
                format!("{}{}", exp_ret.content, store_ir)
            }
            Stmt::Decl(typ, decls) => {
                let mut content = String::new();
                match typ {
                    Type::Var(_t) => {
                        for decl in decls {
                            let ptr_name = bg.new_variable(decl.ident.clone());
                            content.push_str(&format!("\t{} = alloc i32\n", ptr_name));
                            if let Some(init) = &decl.init {
                                let init_ret = init.to_koopa(bg);
                                content.push_str(&init_ret.content);
                                content.push_str(&format!("\tstore {}, {}\n", init_ret.value.unwrap(), ptr_name));
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
            Stmt::If(exp, stmt) => {
                let exp_ret = exp.to_koopa(bg);
                let cond = exp_ret.value.unwrap();
                let br_name = bg.new_branch();
                let then_branch = format!("%{}_then", br_name);
                let merge_branch = next_bb.clone().unwrap_or_else(|| format!("%{}_end", br_name));
                let mut ir = String::new();
                ir.push_str(&exp_ret.content);
                ir.push_str(&format!("\tbr {}, {}, {}\n", cond, then_branch, merge_branch));
                ir.push_str(&format!("{}:\n", then_branch));
                bg.set_next_br(merge_branch.clone());
                ir.push_str(&stmt.to_koopa_ir(bg));
                if next_bb.is_none() {
                    ir.push_str(&format!("{}:\n", merge_branch));
                }
                else {
                    ir.push_str(&format!("\tjump {}\n", merge_branch));
                }
                ir
            }
            Stmt::IfElse(exp, then_stmt, else_stmt) => {
                let exp_ret = exp.to_koopa(bg);
                let cond = exp_ret.value.unwrap();
                let br_name = bg.new_branch();
                let then_branch = format!("%{}_then", br_name);
                let else_branch = format!("%{}_else", br_name);
                let merge_branch = next_bb.clone().unwrap_or_else(|| format!("%{}_end", br_name));
                let mut ir = String::new();
                ir.push_str(&exp_ret.content);
                ir.push_str(&format!("\tbr {}, {}, {}\n\n", cond, then_branch, else_branch));
                ir.push_str(&format!("{}:\n", then_branch));
                ir.push_str(&then_stmt.to_koopa_ir(bg));
                ir.push_str(&format!("\tjump {}\n", merge_branch));
                ir.push_str(&format!("{}:\n", else_branch));
                ir.push_str(&else_stmt.to_koopa_ir(bg));
                if next_bb.is_none() {
                    ir.push_str(&format!("{}:\n", merge_branch));
                }
                else {
                    ir.push_str(&format!("\tjump {}\n", merge_branch));
                }
                ir
            }
            Stmt::Return(num) => {
                let ret = num.to_koopa(bg);
                let additional_ir = format!("\tret {}\n", ret.value.unwrap());
                format!("{}{}", ret.content, additional_ir)
            }
            Stmt::Exp(exp) => {
                let exp_ret = exp.to_koopa(bg);
                exp_ret.content
            }
            Stmt::Empty => String::new(),
        }
    }
}

impl Stmt {
    fn may_goto_next(&self) -> bool {
        match self {
            Stmt::Return(_) => false,
            Stmt::If(_, then_stmt) => then_stmt.may_goto_next(),
            Stmt::IfElse(_, then_stmt, else_stmt) => then_stmt.may_goto_next() || else_stmt.may_goto_next(),
            Stmt::Block(stmts) => stmts.iter().any(|stmt| stmt.may_goto_next()),
            Stmt::Assign(_, _) | Stmt::Decl(_, _) | Stmt::Exp(_) | Stmt::Empty => true,
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
                if let Some(const_value) = bg.try_get_constant(name.clone()) {
                    ReturnValue::new(Some(const_value.to_string()), String::new())
                } else {
                    let ptr_name = bg.get_variable(name.clone());
                    let loaded_name = bg.next_temp();
                    let load_ir = format!("\t{} = load {}\n", loaded_name, ptr_name);
                    ReturnValue::new(Some(loaded_name), load_ir)
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
                bg.try_get_constant(name.clone())
            }
        }
    }
}
