use std::{collections::HashMap, fmt::Debug};
use crate::ast_tool::{eval_binary_const, eval_unary_const, gen_binary_koopa_ir, gen_unary_koopa_ir};
use crate::koopa::{KoopaLine, KoopaLines};
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
        let branch_name = format!("%br{}", self.branch_counter);
        self.branch_counter += 1;
        branch_name
    }

    pub fn next_bb(&self) -> Option<String> {
        self.next_bb.clone()
    }

    pub fn new_next_bb_if_none(&mut self) {
        if self.next_bb.is_none() {
            let new_bb = self.new_branch();
            self.next_bb = Some(new_bb);
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReturnValue {
    pub value: Option<String>,
    pub content: KoopaLines,
}

impl ReturnValue {
    pub fn new(value: Option<String>, content: KoopaLines) -> Self {
        Self { value, content }
    }
    pub fn with_content(content: KoopaLines) -> Self {
        Self { value: None, content }
    }
    pub fn extend(mut self, additional: ReturnValue) -> Self {
        self.content.add_lines(additional.content);
        self.value = additional.value;
        self
    }
}

#[allow(unused)]
pub trait AstNode: Debug {
    fn to_koopa(&self, bg: &mut Background) -> ReturnValue {
        ReturnValue::with_content(KoopaLines::new())
    }
    fn to_koopa_lines(&self, bg: &mut Background) -> KoopaLines {
        self.to_koopa(bg).content
    }
}

impl AstNode for CompUnit {
    fn to_koopa_lines(&self, bg: &mut Background) -> KoopaLines {
        self.func_def.to_koopa_lines(bg)
    }
}

impl AstNode for FuncDef {
    fn to_koopa_lines(&self, bg: &mut Background) -> KoopaLines {
        let mut lines = KoopaLines::new();
        lines.add_line(KoopaLine::FuncStart(self.ident.clone(), self.func_type.to_koopa_type()));
        lines.add_line(KoopaLine::Label("%entry".to_string()));
        lines.add_lines(self.block.to_koopa_lines(bg));
        lines.remove_last_label();
        lines.add_line(KoopaLine::FuncEnd);
        lines
    }
}

impl FuncType {
    fn to_koopa_type(&self) -> String {
        match self {
            FuncType::Void => "void".to_string(),
            FuncType::Int => "i32".to_string(),
        }
    }
}

impl AstNode for Vec<Stmt> {
    fn to_koopa_lines(&self, bg: &mut Background) -> KoopaLines {
        let ts = bg.record();
        bg.increment_time();
        let mut block_lines = KoopaLines::new();
        let next_bb = bg.next_bb.clone();
        // Only the last statement in the block set bb to next_bb. Others set to None.
        for (id, stmt) in self.iter().enumerate() {
            if id == self.len() - 1 && next_bb.is_some() {
                bg.next_bb = next_bb.clone();
                block_lines.add_lines(stmt.to_koopa_lines(bg));
            } else {
                bg.next_bb = None;
                block_lines.add_lines(stmt.to_koopa_lines(bg));
                if let Some(bb) = bg.next_bb.clone() {
                    block_lines.add_line(KoopaLine::Label(bb));
                }
            }
        }
        bg.rollback(ts);
        bg.next_bb = next_bb;
        block_lines
    }
}

impl AstNode for Stmt {
    fn to_koopa_lines(&self, bg: &mut Background) -> KoopaLines {
        // next_bb = None means the next stmts hasn't closed yet.
        match self {
            Stmt::Block(block) => block.to_koopa_lines(bg),
            Stmt::Assign(ident, exp) => {
                let exp_ret = exp.to_koopa(bg);
                let ptr = bg.get_variable(ident.clone());
                let mut content = exp_ret.content;
                content.add_line(KoopaLine::Store(exp_ret.value.unwrap(), ptr));
                content
            }
            Stmt::Decl(typ, decls) => {
                let mut content = KoopaLines::new();
                match typ {
                    Type::Var(_t) => {
                        for decl in decls {
                            let ptr_name = bg.new_variable(decl.ident.clone());
                            content.add_line(KoopaLine::Alloc(ptr_name.clone()));
                            if let Some(init) = &decl.init {
                                let init_ret = init.to_koopa(bg);
                                content.add_lines(init_ret.content);
                                content.add_line(KoopaLine::Store(init_ret.value.unwrap(), ptr_name));
                            }
                        }
                        content
                    }
                    Type::Const(_) => {
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
            Stmt::If(_, _) | Stmt::IfElse(_, _, _) | Stmt::Return(_) => {
                // These methods require the next stmts is closed. So if next_bb = None, we will generate a new bb for the next stmts.
                let mut ir = KoopaLines::new();
                bg.new_next_bb_if_none();
                match self {
                    Stmt::If(exp, stmt) => {
                        let exp_ret = exp.to_koopa(bg);
                        let cond = exp_ret.value.unwrap();
                        let br_name = bg.new_branch();
                        let then_branch = format!("{}_then", br_name);
                        ir.add_lines(exp_ret.content);
                        ir.add_line(KoopaLine::Br(cond, then_branch.clone(), bg.next_bb.clone().unwrap()));
                        // Then Block
                        ir.add_lines(KoopaLines::make_block(stmt.to_koopa_lines(bg), then_branch, bg.next_bb.clone()));
                    }
                    Stmt::IfElse(exp, then_stmt, else_stmt) => {
                        let exp_ret = exp.to_koopa(bg);
                        let cond = exp_ret.value.unwrap();
                        let br_name = bg.new_branch();
                        let then_branch = format!("{}_then", br_name);
                        let else_branch = format!("{}_else", br_name);
                        ir.add_lines(exp_ret.content);
                        ir.add_line(KoopaLine::Br(cond, then_branch.clone(), else_branch.clone()));
                        ir.add_lines(KoopaLines::make_block(then_stmt.to_koopa_lines(bg), then_branch, bg.next_bb.clone()));
                        ir.add_lines(KoopaLines::make_block(else_stmt.to_koopa_lines(bg), else_branch, bg.next_bb.clone()));
                    }
                    Stmt::Return(num) => {
                        let ret = num.to_koopa(bg);
                        ir.add_lines(ret.content);
                        ir.add_line(KoopaLine::Ret(ret.value.unwrap()));
                    }
                    _ => unreachable!(),
                }
                ir
            }
            Stmt::Exp(exp) => {
                let exp_ret = exp.to_koopa(bg);
                exp_ret.content
            }
            Stmt::Empty => KoopaLines::new(),
        }
    }
}

impl AstNode for Exp {
    fn to_koopa(&self, bg: &mut Background) -> ReturnValue {
        match self {
            Exp::Number(num) => ReturnValue::new(Some(num.to_string()), KoopaLines::new()),
            Exp::UnaryExp(op, exp) => {
                let exp_ret = exp.to_koopa(bg);
                let src = exp_ret.value.clone().unwrap();
                exp_ret.extend(
                    gen_unary_koopa_ir(op, src, bg)
                )
            }
            Exp::BinaryExp(op, left, right) => {
                gen_binary_koopa_ir(op, left, right, bg)
            }
            Exp::Ident(name) => {
                if let Some(const_value) = bg.try_get_constant(name.clone()) {
                    ReturnValue::new(Some(const_value.to_string()), KoopaLines::new())
                } else {
                    let ptr_name = bg.get_variable(name.clone());
                    let loaded_name = bg.next_temp();
                    let mut content = KoopaLines::new();
                    content.add_line(KoopaLine::Load(loaded_name.clone(), ptr_name));
                    ReturnValue::new(Some(loaded_name), content)
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
