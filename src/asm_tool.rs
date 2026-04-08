use koopa::ir::dfg::DataFlowGraph;
use koopa::ir::values::{Binary, BinaryOp};
use koopa::ir::{Value, ValueKind};
use koopa::ir::{FunctionData, Program};
use crate::asm::{Asm, Background};
use crate::riscv::{AsmLine, AsmValue, RegAddress, RegLocation, RegName, RegisterAllocator};

/// Transfer a value to a register, generating necessary instructions if needed.
pub fn value_to_reg(value: Value, reg: RegName, bg: &Background, asm: &mut Asm) {
    let inst = bg.get_value(value);
    match inst.kind() {
        ValueKind::Integer(i) => {
            asm.add_line(AsmLine::Li(reg, i.value()));
        }
        _ => {
            panic!("Unsupported value kind for value_to_reg: {:?}", inst.kind());
        }
    }
}

pub fn regaddress_to_regname(addr: &RegAddress, _asm: &mut Asm) -> RegName {
    match addr.location.clone() {
        RegLocation::Reg(reg) => reg,
        RegLocation::Stack(_) => panic!("Cannot convert stack slot to register"),
    }
}