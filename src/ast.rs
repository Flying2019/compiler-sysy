use std::fmt::Debug;
use crate::ast_tool::{Background, eval_binary_const, eval_unary_const, gen_binary_koopa_ir, gen_unary_koopa_ir};
use crate::koopa::{KoopaLine, KoopaLines};
use crate::lalr::*;

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
        let next_bb = bg.next_bb().clone();
        // Only the last statement in the block set bb to next_bb. Others set to None.
        for (id, stmt) in self.iter().enumerate() {
            if id == self.len() - 1 && next_bb.is_some() {
                bg.set_next_bb(next_bb.clone());
                block_lines.add_lines(stmt.to_koopa_lines(bg));
            } else {
                bg.set_next_bb(None);
                block_lines.add_lines(stmt.to_koopa_lines(bg));
                if let Some(bb) = bg.next_bb().clone() {
                    block_lines.add_line(KoopaLine::Label(bb));
                }
            }
        }
        bg.rollback(ts);
        bg.set_next_bb(next_bb);
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
            Stmt::If(_, _) | Stmt::IfElse(_, _, _) | Stmt::Return(_) | Stmt::While(_, _) | Stmt::Continue | Stmt::Break => {
                // These methods require a closed basic block.
                let mut ir = KoopaLines::new();
                bg.new_next_bb_if_none();
                match self {
                    Stmt::If(exp, stmt) => {
                        let exp_ret = exp.to_koopa(bg);
                        let cond = exp_ret.value.unwrap();
                        let bb_name = bg.new_bb();
                        let then_branch = format!("{}_then", bb_name);
                        ir.add_lines(exp_ret.content);
                        ir.add_line(KoopaLine::Br(cond, then_branch.clone(), bg.next_bb().clone().unwrap()));
                        // Then Block
                        ir.add_lines(KoopaLines::make_block(stmt.to_koopa_lines(bg), then_branch, bg.next_bb().clone()));
                    }
                    Stmt::IfElse(exp, then_stmt, else_stmt) => {
                        let exp_ret = exp.to_koopa(bg);
                        let cond = exp_ret.value.unwrap();
                        let bb_name = bg.new_bb();
                        let then_branch = format!("{}_then", bb_name);
                        let else_branch = format!("{}_else", bb_name);
                        ir.add_lines(exp_ret.content);
                        ir.add_line(KoopaLine::Br(cond, then_branch.clone(), else_branch.clone()));
                        ir.add_lines(KoopaLines::make_block(then_stmt.to_koopa_lines(bg), then_branch, bg.next_bb().clone()));
                        ir.add_lines(KoopaLines::make_block(else_stmt.to_koopa_lines(bg), else_branch, bg.next_bb().clone()));
                    }
                    Stmt::While(exp, stmt) => {
                        let old_entry = bg.get_loop_entry();
                        let old_next = bg.get_loop_next();
                        let bb_name = bg.new_bb();
                        let entry_bb = format!("{}_entry", bb_name);
                        let body_bb = format!("{}_body", bb_name);
                        ir.add_line(KoopaLine::Jump(entry_bb.clone()));
                        ir.add_line(KoopaLine::Label(entry_bb.clone()));
                        let exp_ret = exp.to_koopa(bg);
                        ir.add_lines(exp_ret.content);
                        bg.set_loop(bg.next_bb(), Some(entry_bb.clone()));
                        let cond = exp_ret.value.unwrap();
                        ir.add_line(KoopaLine::Br(cond, body_bb.clone(), bg.next_bb().clone().unwrap()));
                        let old_next_bb = bg.next_bb();
                        bg.set_next_bb(Some(entry_bb.clone()));
                        ir.add_lines(KoopaLines::make_block(stmt.to_koopa_lines(bg), body_bb, Some(entry_bb)));
                        bg.set_next_bb(old_next_bb);
                        bg.set_loop(old_next, old_entry);
                    }
                    Stmt::Return(num) => {
                        let ret = num.to_koopa(bg);
                        ir.add_lines(ret.content);
                        ir.add_line(KoopaLine::Ret(ret.value.unwrap()));
                    }
                    Stmt::Break => {
                        ir.add_line(KoopaLine::Jump(bg.get_loop_next().unwrap()));
                    }
                    Stmt::Continue => {
                        ir.add_line(KoopaLine::Jump(bg.get_loop_entry().unwrap()));
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
