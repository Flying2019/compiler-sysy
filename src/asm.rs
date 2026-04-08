use std::collections::{HashMap, HashSet};

use koopa::ir::dfg::DataFlowGraph;
use koopa::ir::entities::ValueData;
use koopa::ir::values::{Binary, BinaryOp};
use koopa::ir::{Value, ValueKind};
use koopa::ir::{FunctionData, Program};
use crate::riscv::{AsmLine, RegAddress, RegName, RegisterAllocator};
use crate::asm_tool::regaddress_to_regname;

fn name_to_symbol(name: &str) -> String {
    if name.starts_with("@") {
        name[1..].to_string()
    } else {
        name.to_string()
    }
}

#[allow(dead_code)]
pub struct Asm {
    content: Vec<String>,
}

impl Asm {
    pub fn new() -> Self {
        Self { content: Vec::new() }
    }

    pub fn add_asm(&mut self, func_asm: Asm) {
        self.content.extend(func_asm.content);
    }

    pub fn add_text(&mut self, line: String) {
        self.content.push(line);
    }

    pub fn add_line(&mut self, line: AsmLine) {
        self.content.push(line.to_string());
    }

    pub fn to_string(&self) -> String {
        self.content.join("\n")
    }
}

pub struct Background<'a> {
    prog: Option<&'a Program>,
    dfg: Option<&'a DataFlowGraph>,
}

impl<'a> Background<'a> {
    pub fn new() -> Self {
        Self {
            prog: None,
            dfg: None,
        }
    }

    pub fn with_prog<'b>(&self, prog: &'b Program) -> Background<'b>
        where 'a: 'b,
    {
        Background { prog: Some(prog), ..self.clone() }
    }

    pub fn with_dfg<'b>(&self, dfg: &'b DataFlowGraph) -> Background<'b>
        where 'a: 'b,
    {
        Background { dfg: Some(dfg), ..self.clone() }
    }

    pub fn get_value(&self, value: Value) -> &ValueData {
        self.dfg.unwrap().value(value)
    }
}

impl<'a> Clone for Background<'a> {
    fn clone(&self) -> Self {
        Self {
            prog: self.prog,
            dfg: self.dfg,
        }
    }
}

pub trait GenerateAsm {
    fn to_asm(&self, bg: &Background) -> Asm;
}

impl GenerateAsm for Program {
    fn to_asm(&self, bg: &Background) -> Asm {
        let mut result = Asm::new();
        for &func in self.func_layout() {
            result.add_asm(self.func(func).to_asm(&bg.with_prog(&self)));
        }
        result
    }
}

struct TempRecord {
    loc: RegAddress,
    used_by: HashSet<Value>,
}

impl TempRecord {
    pub fn from_value(value: Value, bg: &Background, loc: RegAddress) -> TempRecord {
        TempRecord {
            loc: loc,
            used_by: bg.get_value(value).used_by().clone(),
        }
    }
}

fn consume_temp_reg(
    operand: Value,
    user: Value,
    temp_value: &mut HashMap<Value, TempRecord>,
    asm: &mut Asm,
) -> Option<RegName> {
    let mut should_release = false;
    let mut reg = None;
    if let Some(record) = temp_value.get_mut(&operand) {
        record.used_by.remove(&user);
        should_release = record.used_by.is_empty();
        reg = Some(regaddress_to_regname(&record.loc, asm));
    }
    if should_release {
        temp_value.remove(&operand);
    }
    reg
}

impl GenerateAsm for &FunctionData {
    fn to_asm(&self, bg: &Background) -> Asm {
        let bg = &bg.with_dfg(&self.dfg());
        let mut result = Asm::new();
        let mut context = Asm::new();
        let mut temp_value = HashMap::new();
        let name = name_to_symbol(self.name());
        result.add_text("\t.text".to_string());
        result.add_text(format!("\t.globl {}", name));
        result.add_text(format!("{}:", name));
        let ra =  RegisterAllocator::new();
        for (&_bb, node) in self.layout().bbs() {
            let insts = node.insts().keys();
            for &inst in insts {
                let ret = inst_to_asm(inst, &bg, &ra, &mut context, &mut temp_value);
                if let Some(ret) = ret {
                    temp_value.insert(inst, TempRecord::from_value(inst, &bg, ret));
                }
            }
        }
        if ra.max_stack_size() > 0 {
            let max_stack_size = ra.max_stack_size();
            result.add_text(format!("\taddi sp, sp, -{}", max_stack_size * 4));
            result.add_asm(context);
            result.add_text(format!("\taddi sp, sp, {}", max_stack_size * 4));
        }
        else {
            result.add_asm(context);
        }
        result.add_line(AsmLine::Ret);
        result
    }
}

fn inst_to_asm(
    value: Value,
    bg: &Background,
    ra: &RegisterAllocator,
    asm: &mut Asm,
    temp_value: &mut HashMap<Value, TempRecord>,
) -> Option<RegAddress> {
    let inst = bg.get_value(value);
    match inst.kind() {
        ValueKind::Return(r) => {
            if let Some(value) = r.value() {
                match bg.get_value(value).kind() {
                    ValueKind::Integer(c) => {
                        asm.add_line(AsmLine::Li(RegName::Ret, c.value()));
                    }
                    _ => {
                        let reg_name = consume_temp_reg(value, value, temp_value, asm).unwrap();
                        asm.add_line(AsmLine::Mv(RegName::Ret, reg_name));
                    }
                }
            }
            None
        }
        ValueKind::Integer(_) => {
            panic!("Integer value should not be directly used as an instruction");
        }
        ValueKind::Binary(binary) => {
            let ret = binary_to_asm(value, binary, bg, ra, asm, temp_value);
            Some(ret)
        }
        _ => unimplemented!()
    }
}

fn operand_to_reg(
    operand: Value,
    user: Value,
    bg: &Background,
    temp_value: &mut HashMap<Value, TempRecord>,
    asm: &mut Asm,
    scratch: RegName,
) -> RegName {
    match bg.get_value(operand).kind() {
        ValueKind::Integer(c) => {
            asm.add_line(AsmLine::Li(scratch.clone(), c.value()));
            scratch
        }
        _ => consume_temp_reg(operand, user, temp_value, asm).unwrap(),
    }
}

fn binary_to_asm(
    value: Value,
    binary: &Binary,
    bg: &Background,
    ra: &RegisterAllocator,
    asm: &mut Asm,
    temp_value: &mut HashMap<Value, TempRecord>,
) -> RegAddress {
    let dest = ra.register(value);
    let dest_reg = regaddress_to_regname(&dest, asm);
    let lhs = operand_to_reg(binary.lhs(), value, bg, temp_value, asm, RegName::TempT(0));
    let rhs = operand_to_reg(binary.rhs(), value, bg, temp_value, asm, RegName::TempT(1));

    match binary.op() {
        BinaryOp::Add => asm.add_line(AsmLine::Add(dest_reg, lhs, rhs)),
        BinaryOp::Sub => asm.add_line(AsmLine::Sub(dest_reg, lhs, rhs)),
        BinaryOp::Mul => asm.add_line(AsmLine::Mul(dest_reg, lhs, rhs)),
        BinaryOp::Div => asm.add_line(AsmLine::Div(dest_reg, lhs, rhs)),
        BinaryOp::Mod => asm.add_line(AsmLine::Rem(dest_reg, lhs, rhs)),
        BinaryOp::Eq => {
            asm.add_line(AsmLine::Xor(dest_reg.clone(), lhs, rhs));
            asm.add_line(AsmLine::Seqz(dest_reg.clone(), dest_reg));
        }
        BinaryOp::NotEq => {
            asm.add_line(AsmLine::Xor(dest_reg.clone(), lhs, rhs));
            asm.add_line(AsmLine::Snez(dest_reg.clone(), dest_reg));
        }
        BinaryOp::Le => {
            asm.add_line(AsmLine::Slt(dest_reg.clone(), rhs, lhs));
            asm.add_line(AsmLine::Xori(dest_reg.clone(), dest_reg, 1));
        }
        BinaryOp::Lt => asm.add_line(AsmLine::Slt(dest_reg, lhs, rhs)),
        BinaryOp::Ge => {
            asm.add_line(AsmLine::Slt(dest_reg.clone(), lhs, rhs));
            asm.add_line(AsmLine::Xori(dest_reg.clone(), dest_reg, 1));
        }
        BinaryOp::Gt => asm.add_line(AsmLine::Slt(dest_reg, rhs, lhs)),
        BinaryOp::And => asm.add_line(AsmLine::And(dest_reg, lhs, rhs)),
        BinaryOp::Or => asm.add_line(AsmLine::Or(dest_reg, lhs, rhs)),
        _ => unimplemented!("{}", format!("Unsupported binary operation: {:?}", binary.op())),
    }
    dest
}