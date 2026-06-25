#![allow(dead_code)]

use crate::lalr::{BinaryOp, Exp, FuncDef, InitVal, Stmt, Type, VarDecl};
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub(crate) struct AsyncCfgFunction {
    pub(crate) entry: usize,
    pub(crate) blocks: Vec<AsyncBlock>,
}

#[derive(Debug, Clone)]
pub(crate) struct AsyncBlock {
    pub(crate) id: usize,
    pub(crate) ops: Vec<AsyncOp>,
    pub(crate) terminator: AsyncTerminator,
}

#[derive(Debug, Clone)]
pub(crate) enum AsyncOp {
    Decl(Type, Vec<crate::lalr::SingleDecl>),
    Assign(Exp, Exp),
    Eval(Exp),
    PromiseWait(Exp),
}

#[derive(Debug, Clone)]
pub(crate) enum AsyncTerminator {
    Return(Option<Exp>),
    Jump(usize),
    Branch {
        cond: Exp,
        then_block: usize,
        else_block: usize,
    },
    Await(AwaitTerminator),
    Unreachable,
}

#[derive(Debug, Clone)]
pub(crate) struct AwaitTerminator {
    pub(crate) state: i32,
    pub(crate) child: Exp,
    pub(crate) result_target: Option<String>,
    pub(crate) resume_block: usize,
    pub(crate) resume_stmt_index: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveAcrossAwait {
    pub(crate) state: i32,
    pub(crate) vars: Vec<String>,
}

impl AsyncCfgFunction {
    pub(crate) fn await_points(&self) -> Vec<&AwaitTerminator> {
        self.blocks
            .iter()
            .filter_map(|block| match &block.terminator {
                AsyncTerminator::Await(await_term) => Some(await_term),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn live_across_awaits(&self) -> Vec<LiveAcrossAwait> {
        let mut live_in = vec![HashSet::<String>::new(); self.blocks.len()];
        let mut live_out = vec![HashSet::<String>::new(); self.blocks.len()];

        loop {
            let mut changed = false;
            for block in self.blocks.iter().rev() {
                let mut next_out = HashSet::new();
                for succ in self.successors(block) {
                    next_out.extend(live_in[succ].iter().cloned());
                }

                let (uses, defs) = block_use_def(block);
                let mut next_in = uses;
                next_in.extend(next_out.difference(&defs).cloned());

                if live_out[block.id] != next_out {
                    live_out[block.id] = next_out;
                    changed = true;
                }
                if live_in[block.id] != next_in {
                    live_in[block.id] = next_in;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        self.blocks
            .iter()
            .filter_map(|block| match &block.terminator {
                AsyncTerminator::Await(await_term) => {
                    let mut vars = live_out[block.id].iter().cloned().collect::<Vec<_>>();
                    vars.sort();
                    Some(LiveAcrossAwait {
                        state: await_term.state,
                        vars,
                    })
                }
                _ => None,
            })
            .collect()
    }

    fn successors(&self, block: &AsyncBlock) -> Vec<usize> {
        match &block.terminator {
            AsyncTerminator::Jump(target) => vec![*target],
            AsyncTerminator::Branch {
                then_block,
                else_block,
                ..
            } => vec![*then_block, *else_block],
            AsyncTerminator::Await(await_term) => vec![await_term.resume_block],
            AsyncTerminator::Return(_) | AsyncTerminator::Unreachable => Vec::new(),
        }
    }
}

pub(crate) fn build_async_cfg(func: &FuncDef) -> AsyncCfgFunction {
    CfgBuilder::new().build(func)
}

pub(crate) fn build_top_level_async_cfg(func: &FuncDef) -> AsyncCfgFunction {
    let mut blocks = Vec::new();
    let mut current_ops = Vec::new();
    let mut state = 1i32;

    for (stmt_index, stmt) in func.block.iter().enumerate() {
        if let Some((child, result_target)) = top_level_await(stmt) {
            let block_id = blocks.len();
            let resume_block = block_id + 1;
            blocks.push(AsyncBlock {
                id: block_id,
                ops: current_ops,
                terminator: AsyncTerminator::Await(AwaitTerminator {
                    state,
                    child,
                    result_target,
                    resume_block,
                    resume_stmt_index: stmt_index + 1,
                }),
            });
            state += 1;
            current_ops = Vec::new();
        } else {
            current_ops.push(AsyncOp::Eval(Exp::Number(0)));
        }
    }

    let block_id = blocks.len();
    blocks.push(AsyncBlock {
        id: block_id,
        ops: current_ops,
        terminator: AsyncTerminator::Return(None),
    });

    AsyncCfgFunction { entry: 0, blocks }
}

struct CfgBuilder {
    blocks: Vec<AsyncBlock>,
    state: i32,
    temp_id: usize,
    loop_stack: Vec<LoopTargets>,
}

#[derive(Debug, Clone)]
struct LoopTargets {
    break_block: usize,
    continue_block: usize,
}

impl CfgBuilder {
    fn new() -> Self {
        Self {
            blocks: Vec::new(),
            state: 1,
            temp_id: 0,
            loop_stack: Vec::new(),
        }
    }

    fn build(mut self, func: &FuncDef) -> AsyncCfgFunction {
        let entry = self.new_block();
        let current = self.lower_stmts(entry, &func.block);
        self.terminate_if_open(current, AsyncTerminator::Return(None));
        AsyncCfgFunction {
            entry,
            blocks: self.blocks,
        }
    }

    fn new_block(&mut self) -> usize {
        let id = self.blocks.len();
        self.blocks.push(AsyncBlock {
            id,
            ops: Vec::new(),
            terminator: AsyncTerminator::Unreachable,
        });
        id
    }

    fn is_open(&self, block: usize) -> bool {
        matches!(self.blocks[block].terminator, AsyncTerminator::Unreachable)
    }

    fn terminate_if_open(&mut self, block: usize, term: AsyncTerminator) {
        if self.is_open(block) {
            self.blocks[block].terminator = term;
        }
    }

    fn push_op(&mut self, block: usize, op: AsyncOp) {
        if self.is_open(block) {
            self.blocks[block].ops.push(op);
        }
    }

    fn lower_stmts(&mut self, mut current: usize, stmts: &[Stmt]) -> usize {
        for stmt in stmts {
            if !self.is_open(current) {
                return current;
            }
            current = self.lower_stmt(current, stmt);
        }
        current
    }

    fn lower_stmt(&mut self, current: usize, stmt: &Stmt) -> usize {
        match stmt {
            Stmt::Block(stmts) => self.lower_stmts(current, stmts),
            Stmt::Decl(ty, decls) => {
                let mut current = current;
                let mut lowered = Vec::new();
                for decl in decls {
                    let init = decl
                        .init
                        .as_ref()
                        .map(|init| self.lower_init(&mut current, init));
                    lowered.push(crate::lalr::SingleDecl {
                        var: decl.var.clone(),
                        init,
                    });
                }
                self.push_op(current, AsyncOp::Decl(ty.clone(), lowered));
                current
            }
            Stmt::Assign(lhs, rhs) => {
                let mut current = current;
                let lhs = self.lower_exp(&mut current, lhs);
                let rhs = self.lower_exp(&mut current, rhs);
                self.push_op(current, AsyncOp::Assign(lhs, rhs));
                current
            }
            Stmt::Exp(exp) => {
                let mut current = current;
                if let Exp::Await(inner) = exp {
                    current = self.suspend_for_await(current, *inner.clone(), None, 0);
                } else {
                    let exp = self.lower_exp(&mut current, exp);
                    self.push_op(current, AsyncOp::Eval(exp));
                }
                current
            }
            Stmt::PromiseWait(exp) => {
                let mut current = current;
                let exp = self.lower_exp(&mut current, exp);
                self.push_op(current, AsyncOp::PromiseWait(exp));
                current
            }
            Stmt::Return(exp) => {
                let mut current = current;
                let exp = exp.as_ref().map(|exp| self.lower_exp(&mut current, exp));
                self.terminate_if_open(current, AsyncTerminator::Return(exp));
                current
            }
            Stmt::If(cond, then_stmt) => {
                let then_block = self.new_block();
                let end_block = self.new_block();
                let current = self.lower_cond(current, cond, then_block, end_block);
                self.terminate_if_open(current, AsyncTerminator::Jump(end_block));
                let then_end = self.lower_stmt(then_block, then_stmt);
                self.terminate_if_open(then_end, AsyncTerminator::Jump(end_block));
                end_block
            }
            Stmt::IfElse(cond, then_stmt, else_stmt) => {
                let then_block = self.new_block();
                let else_block = self.new_block();
                let end_block = self.new_block();
                let current = self.lower_cond(current, cond, then_block, else_block);
                self.terminate_if_open(current, AsyncTerminator::Jump(end_block));
                let then_end = self.lower_stmt(then_block, then_stmt);
                self.terminate_if_open(then_end, AsyncTerminator::Jump(end_block));
                let else_end = self.lower_stmt(else_block, else_stmt);
                self.terminate_if_open(else_end, AsyncTerminator::Jump(end_block));
                end_block
            }
            Stmt::While(cond, body) => {
                let cond_block = self.new_block();
                let body_block = self.new_block();
                let end_block = self.new_block();
                self.terminate_if_open(current, AsyncTerminator::Jump(cond_block));
                self.loop_stack.push(LoopTargets {
                    break_block: end_block,
                    continue_block: cond_block,
                });
                let cond_end = self.lower_cond(cond_block, cond, body_block, end_block);
                self.terminate_if_open(cond_end, AsyncTerminator::Jump(end_block));
                let body_end = self.lower_stmt(body_block, body);
                self.terminate_if_open(body_end, AsyncTerminator::Jump(cond_block));
                self.loop_stack.pop();
                end_block
            }
            Stmt::Break => {
                if let Some(targets) = self.loop_stack.last() {
                    self.terminate_if_open(current, AsyncTerminator::Jump(targets.break_block));
                }
                current
            }
            Stmt::Continue => {
                if let Some(targets) = self.loop_stack.last() {
                    self.terminate_if_open(current, AsyncTerminator::Jump(targets.continue_block));
                }
                current
            }
            Stmt::Empty => current,
        }
    }

    fn lower_init(&mut self, current: &mut usize, init: &InitVal) -> InitVal {
        match init {
            InitVal::Exp(exp) => InitVal::Exp(self.lower_exp(current, exp)),
            InitVal::Arr(items) => InitVal::Arr(
                items
                    .iter()
                    .map(|item| self.lower_init(current, item))
                    .collect(),
            ),
        }
    }

    fn lower_cond(
        &mut self,
        current: usize,
        cond: &Exp,
        then_block: usize,
        else_block: usize,
    ) -> usize {
        match cond {
            Exp::BinaryExp(BinaryOp::And, lhs, rhs) => {
                let rhs_block = self.new_block();
                let current = self.lower_cond(current, lhs, rhs_block, else_block);
                self.terminate_if_open(current, AsyncTerminator::Jump(else_block));
                self.lower_cond(rhs_block, rhs, then_block, else_block)
            }
            Exp::BinaryExp(BinaryOp::Or, lhs, rhs) => {
                let rhs_block = self.new_block();
                let current = self.lower_cond(current, lhs, then_block, rhs_block);
                self.terminate_if_open(current, AsyncTerminator::Jump(then_block));
                self.lower_cond(rhs_block, rhs, then_block, else_block)
            }
            other => {
                let mut current = current;
                let cond = self.lower_exp(&mut current, other);
                self.terminate_if_open(
                    current,
                    AsyncTerminator::Branch {
                        cond,
                        then_block,
                        else_block,
                    },
                );
                current
            }
        }
    }

    fn lower_exp(&mut self, current: &mut usize, exp: &Exp) -> Exp {
        match exp {
            Exp::Await(inner) => {
                let child = self.lower_exp(current, inner);
                let temp = self.next_await_temp();
                *current = self.suspend_for_await(*current, child, Some(temp.clone()), 0);
                Exp::Ident(temp)
            }
            Exp::UnaryExp(op, inner) => {
                Exp::UnaryExp(op.clone(), Box::new(self.lower_exp(current, inner)))
            }
            Exp::BinaryExp(op, lhs, rhs) => Exp::BinaryExp(
                op.clone(),
                Box::new(self.lower_exp(current, lhs)),
                Box::new(self.lower_exp(current, rhs)),
            ),
            Exp::Sleep(duration) => Exp::Sleep(Box::new(self.lower_exp(current, duration))),
            Exp::PromiseWait(inner) => Exp::PromiseWait(Box::new(self.lower_exp(current, inner))),
            Exp::FuncCall(name, args) => Exp::FuncCall(
                name.clone(),
                args.iter()
                    .map(|arg| self.lower_exp(current, arg))
                    .collect(),
            ),
            Exp::ArrGet(base, index) => Exp::ArrGet(
                Box::new(self.lower_exp(current, base)),
                Box::new(self.lower_exp(current, index)),
            ),
            Exp::Field(base, field) => {
                Exp::Field(Box::new(self.lower_exp(current, base)), field.clone())
            }
            Exp::PtrField(base, field) => {
                Exp::PtrField(Box::new(self.lower_exp(current, base)), field.clone())
            }
            Exp::Number(_) | Exp::New(_) | Exp::Ident(_) => exp.clone(),
        }
    }

    fn next_await_temp(&mut self) -> String {
        let name = format!("__sysy_cfg_await_tmp_{}", self.temp_id);
        self.temp_id += 1;
        name
    }

    fn suspend_for_await(
        &mut self,
        current: usize,
        child: Exp,
        result_target: Option<String>,
        resume_stmt_index: usize,
    ) -> usize {
        let resume_block = self.new_block();
        let state = self.state;
        self.state += 1;
        self.terminate_if_open(
            current,
            AsyncTerminator::Await(AwaitTerminator {
                state,
                child,
                result_target,
                resume_block,
                resume_stmt_index,
            }),
        );
        resume_block
    }
}

fn top_level_await(stmt: &Stmt) -> Option<(Exp, Option<String>)> {
    match stmt {
        Stmt::Decl(_, decls) => decls.iter().find_map(|decl| {
            if let Some(InitVal::Exp(Exp::Await(inner))) = &decl.init {
                Some(((**inner).clone(), Some(var_decl_name(&decl.var))))
            } else {
                None
            }
        }),
        Stmt::Assign(Exp::Ident(name), Exp::Await(inner)) => {
            Some(((**inner).clone(), Some(name.clone())))
        }
        Stmt::Exp(Exp::Await(inner)) => Some(((**inner).clone(), None)),
        Stmt::Return(Some(Exp::Await(inner))) => {
            Some(((**inner).clone(), Some("__return".to_string())))
        }
        _ => None,
    }
}

fn var_decl_name(var: &VarDecl) -> String {
    match var {
        VarDecl::Ident(name) => name.clone(),
        VarDecl::Array(inner, _) => var_decl_name(inner),
    }
}

fn block_use_def(block: &AsyncBlock) -> (HashSet<String>, HashSet<String>) {
    let mut uses = HashSet::new();
    let mut defs = HashSet::new();
    for op in &block.ops {
        match op {
            AsyncOp::Decl(_, decls) => {
                for decl in decls {
                    if let Some(init) = &decl.init {
                        collect_uses_init(init, &mut uses, &defs);
                    }
                    defs.insert(var_decl_name(&decl.var));
                    collect_uses_var_decl_dims(&decl.var, &mut uses, &defs);
                }
            }
            AsyncOp::Assign(lhs, rhs) => {
                collect_uses_exp(rhs, &mut uses, &defs);
                collect_lvalue_uses(lhs, &mut uses, &defs);
                if let Exp::Ident(name) = lhs {
                    defs.insert(name.clone());
                }
            }
            AsyncOp::Eval(exp) | AsyncOp::PromiseWait(exp) => {
                collect_uses_exp(exp, &mut uses, &defs);
            }
        }
    }
    match &block.terminator {
        AsyncTerminator::Return(Some(exp)) => collect_uses_exp(exp, &mut uses, &defs),
        AsyncTerminator::Branch { cond, .. } => collect_uses_exp(cond, &mut uses, &defs),
        AsyncTerminator::Await(await_term) => {
            collect_uses_exp(&await_term.child, &mut uses, &defs);
            if let Some(target) = &await_term.result_target {
                defs.insert(target.clone());
            }
        }
        AsyncTerminator::Return(None) | AsyncTerminator::Jump(_) | AsyncTerminator::Unreachable => {
        }
    }
    (uses, defs)
}

fn collect_uses_init(init: &InitVal, uses: &mut HashSet<String>, defs: &HashSet<String>) {
    match init {
        InitVal::Exp(exp) => collect_uses_exp(exp, uses, defs),
        InitVal::Arr(items) => {
            for item in items {
                collect_uses_init(item, uses, defs);
            }
        }
    }
}

fn collect_lvalue_uses(exp: &Exp, uses: &mut HashSet<String>, defs: &HashSet<String>) {
    match exp {
        Exp::Ident(_) => {}
        other => collect_uses_exp(other, uses, defs),
    }
}

fn collect_uses_var_decl_dims(var: &VarDecl, uses: &mut HashSet<String>, defs: &HashSet<String>) {
    match var {
        VarDecl::Ident(_) => {}
        VarDecl::Array(inner, dim) => {
            collect_uses_var_decl_dims(inner, uses, defs);
            collect_uses_exp(dim, uses, defs);
        }
    }
}

fn collect_uses_exp(exp: &Exp, uses: &mut HashSet<String>, defs: &HashSet<String>) {
    match exp {
        Exp::Ident(name) => {
            if !defs.contains(name) {
                uses.insert(name.clone());
            }
        }
        Exp::UnaryExp(_, inner)
        | Exp::Await(inner)
        | Exp::Sleep(inner)
        | Exp::PromiseWait(inner) => {
            collect_uses_exp(inner, uses, defs);
        }
        Exp::BinaryExp(_, lhs, rhs) | Exp::ArrGet(lhs, rhs) => {
            collect_uses_exp(lhs, uses, defs);
            collect_uses_exp(rhs, uses, defs);
        }
        Exp::FuncCall(_, args) => {
            for arg in args {
                collect_uses_exp(arg, uses, defs);
            }
        }
        Exp::Field(base, _) | Exp::PtrField(base, _) => collect_uses_exp(base, uses, defs),
        Exp::Number(_) | Exp::New(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lalr::{BType, FuncParam};

    #[test]
    fn cfg_splits_loop_and_short_circuit_awaits() {
        let func = FuncDef {
            is_async: true,
            func_type: Type::BType(BType::I32),
            ident: "f".to_string(),
            func_params: Vec::<FuncParam>::new(),
            block: vec![
                Stmt::While(
                    Exp::Await(Box::new(call("tick"))),
                    Box::new(Stmt::Block(vec![Stmt::If(
                        Exp::BinaryExp(
                            BinaryOp::And,
                            Box::new(Exp::Await(Box::new(call("lhs")))),
                            Box::new(Exp::Await(Box::new(call("rhs")))),
                        ),
                        Box::new(Stmt::Break),
                    )])),
                ),
                Stmt::Return(Some(Exp::Number(0))),
            ],
        };

        let cfg = build_async_cfg(&func);
        let await_count = cfg
            .blocks
            .iter()
            .filter(|block| matches!(block.terminator, AsyncTerminator::Await(_)))
            .count();
        let branch_count = cfg
            .blocks
            .iter()
            .filter(|block| matches!(block.terminator, AsyncTerminator::Branch { .. }))
            .count();
        let jump_count = cfg
            .blocks
            .iter()
            .filter(|block| matches!(block.terminator, AsyncTerminator::Jump(_)))
            .count();

        assert_eq!(await_count, 3);
        assert!(branch_count >= 2);
        assert!(jump_count >= 2);
    }

    #[test]
    fn cfg_liveness_tracks_values_used_after_await() {
        let func = FuncDef {
            is_async: true,
            func_type: Type::BType(BType::I32),
            ident: "f".to_string(),
            func_params: Vec::<FuncParam>::new(),
            block: vec![
                Stmt::Decl(
                    Type::BType(BType::I32),
                    vec![crate::lalr::SingleDecl {
                        var: VarDecl::Ident("x".to_string()),
                        init: Some(InitVal::Exp(Exp::Number(7))),
                    }],
                ),
                Stmt::Exp(Exp::Await(Box::new(call("tick")))),
                Stmt::Return(Some(Exp::Ident("x".to_string()))),
            ],
        };

        let cfg = build_async_cfg(&func);
        let live = cfg.live_across_awaits();

        assert_eq!(live.len(), 1);
        assert_eq!(live[0].state, 1);
        assert_eq!(live[0].vars, vec!["x".to_string()]);
    }

    fn call(name: &str) -> Exp {
        Exp::FuncCall(name.to_string(), Vec::new())
    }
}
