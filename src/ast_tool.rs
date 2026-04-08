use crate::{ast::{Background, ReturnValue}, lalr::{BinaryOp, UnaryOp}};


pub fn gen_unary_koopa_ir(op: &UnaryOp, src: String, background: &mut Background) -> ReturnValue {
    let dst = background.next_temp();
    let ir =
    match op {
        UnaryOp::Pos => format!("  {} = add 0, {}\n", dst, src),
        UnaryOp::Neg => format!("  {} = sub 0, {}\n", dst, src),
        UnaryOp::Not => format!("  {} = eq {}, 0\n", dst, src),
    };
    ReturnValue { value: Some(dst), content: ir }
}

pub fn eval_unary_const(op: &UnaryOp, value: i32) -> i32 {
    match op {
        UnaryOp::Pos => value,
        UnaryOp::Neg => -value,
        UnaryOp::Not => if value == 0 { 1 } else { 0 },
    }
}

pub fn gen_binary_koopa_ir(op: &BinaryOp, lhs: String, rhs: String, background: &mut Background) -> ReturnValue {
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