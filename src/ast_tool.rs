use crate::{ast::{AstNode, Background, ReturnValue}, lalr::{BinaryOp, Exp, UnaryOp}};
use crate::koopa::{KoopaLine, KoopaLines};


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
    match op {
        BinaryOp::And | BinaryOp::Or => {
            // These logical operations require short-circuit evaluation, so we need to generate more complex IR.
            unimplemented!()
        }
        _ => {
            let dst = bg.next_temp();
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
            let lhs_ret = lhs.to_koopa(bg);
            let rhs_ret = rhs.to_koopa(bg);
            let lhs_src = lhs_ret.value.clone().unwrap();
            let rhs_src = rhs_ret.value.clone().unwrap();
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