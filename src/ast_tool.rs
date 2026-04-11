use std::collections::HashMap;

use crate::{ast::{AstNode, ReturnValue}, lalr::{BinaryOp, Exp, UnaryOp}};
use crate::koopa::{KoopaLine, KoopaLines};

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

    pub fn set_next_bb(&mut self, bb: Option<String>) {
        self.next_bb = bb;
    }

    pub fn new_next_bb_if_none(&mut self) {
        if self.next_bb.is_none() {
            let new_bb = self.new_branch();
            self.next_bb = Some(new_bb);
        }
    }
}

pub fn gen_unary_koopa_ir(op: &UnaryOp, src: String, background: &mut Background) -> ReturnValue {
    let dst = background.next_temp();
    let line = match op {
        UnaryOp::Pos => KoopaLine::Binary(dst.clone(), "add".to_string(), "0".to_string(), src),
        UnaryOp::Neg => KoopaLine::Binary(dst.clone(), "sub".to_string(), "0".to_string(), src),
        UnaryOp::Not => KoopaLine::Binary(dst.clone(), "eq".to_string(), src, "0".to_string()),
    };
    ReturnValue { value: Some(dst), content: KoopaLines::with_line(line) }
}

pub fn eval_unary_const(op: &UnaryOp, value: i32) -> i32 {
    match op {
        UnaryOp::Pos => value,
        UnaryOp::Neg => -value,
        UnaryOp::Not => if value == 0 { 1 } else { 0 },
    }
}

pub fn gen_binary_koopa_ir(op: &BinaryOp, lhs: &Box<Exp>, rhs: &Box<Exp>, bg: &mut Background) -> ReturnValue {
    let dst = bg.next_temp();
    let lhs_ret = lhs.to_koopa(bg);
    let rhs_ret = rhs.to_koopa(bg);
    let lhs_src = lhs_ret.value.clone().unwrap();
    let rhs_src = rhs_ret.value.clone().unwrap();
    match op {
        BinaryOp::And | BinaryOp::Or => {
            // These logical operations require short-circuit evaluation, so we need to generate more complex IR.
            let mut ir = lhs_ret.content;
            
            let br_label = bg.new_branch();
            let br_then = format!("{}_then", br_label);
            let br_else = format!("{}_else", br_label);
            let br_end = format!("{}_end", br_label);
            // Using ArgLabel to implement SSA form for short-circuit evaluation
            if matches!(op, BinaryOp::And) {
                ir.add_line(KoopaLine::Br(lhs_src.clone(), br_then.clone(), br_else.clone()));
            } else {
                ir.add_line(KoopaLine::Br(lhs_src.clone(), br_else.clone(), br_then.clone()));
            }
            // Then block
            ir.add_line(KoopaLine::Label(br_then));
            ir.add_lines(rhs_ret.content);
            let tmp = bg.next_temp();
            ir.add_lines(KoopaLines::with_line(KoopaLine::Binary(tmp.clone(), "ne".to_string(), rhs_src.clone(), "0".to_string())));
            ir.add_line(KoopaLine::ArgJump(br_end.clone(), tmp));
            // Else block
            ir.add_line(KoopaLine::Label(br_else));
            if matches!(op, BinaryOp::And) {
                ir.add_line(KoopaLine::ArgJump(br_end.clone(), 0.to_string()));
            } else {
                ir.add_line(KoopaLine::ArgJump(br_end.clone(), 1.to_string()));
            }
            // End block
            ir.add_line(KoopaLine::ArgLabel(br_end, dst.clone(), "i32".to_string()));
            ReturnValue { value: Some(dst), content: ir }
        }
        _ => {
            let op_name = match op {
                BinaryOp::Add => "add",
                BinaryOp::Sub => "sub",
                BinaryOp::Mul => "mul",
                BinaryOp::Div => "div",
                BinaryOp::Mod => "mod",
                BinaryOp::Lt => "lt",
                BinaryOp::Gt => "gt",
                BinaryOp::Le => "le",
                BinaryOp::Ge => "ge",
                BinaryOp::Eq => "eq",
                BinaryOp::Ne => "ne",
                _ => unimplemented!("Unsupported binary operation: {:?}", op),
            };
            let mut ir = lhs_ret.content;
            ir.add_lines(rhs_ret.content);
            ir.add_line(KoopaLine::Binary(dst.clone(), op_name.to_string(), lhs_src, rhs_src));
            ReturnValue { value: Some(dst), content: ir }
        }
    }
}

pub fn eval_binary_const(op: &BinaryOp, lhs: i32, rhs: i32) -> i32 {
    match op {
        BinaryOp::Add => lhs + rhs,
        BinaryOp::Sub => lhs - rhs,
        BinaryOp::Mul => lhs * rhs,
        BinaryOp::Div => lhs / rhs,
        BinaryOp::Mod => lhs % rhs,
        BinaryOp::Lt => if lhs < rhs { 1 } else { 0 },
        BinaryOp::Gt => if lhs > rhs { 1 } else { 0 },
        BinaryOp::Le => if lhs <= rhs { 1 } else { 0 },
        BinaryOp::Ge => if lhs >= rhs { 1 } else { 0 },
        BinaryOp::Eq => if lhs == rhs { 1 } else { 0 },
        BinaryOp::Ne => if lhs != rhs { 1 } else { 0 },
        BinaryOp::And => if (lhs != 0) && (rhs != 0) { 1 } else { 0 },
        BinaryOp::Or => if (lhs != 0) || (rhs != 0) { 1 } else { 0 },
    }
}