use std::collections::{HashMap, HashSet};
use koopa::ir::dfg::DataFlowGraph;
use koopa::ir::entities::ValueData;
use koopa::ir::values::{Binary, BinaryOp};
use koopa::ir::{Value, ValueKind};
use koopa::ir::{FunctionData, Program};
use crate::riscv::{AsmLine, AsmValue, RegAddress, RegLocation, RegName, RegisterAllocator};

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
        println!("Creating TempRecord for value {:?}, used by: {:?}", value, bg.get_value(value).used_by());
        TempRecord {
            loc: loc,
            used_by: bg.get_value(value).used_by().clone(),
        }
    }
}

struct ConsumedTemp {
    reg: RegName,
    _holds: Vec<RegAddress>,
}

impl ConsumedTemp {
    fn new(reg: RegName, holds: Vec<RegAddress>) -> Self {
        Self { reg, _holds: holds }
    }

    fn reg(&self) -> RegName {
        self.reg.clone()
    }
}

impl Into<ConsumedTemp> for RegName {
    fn into(self) -> ConsumedTemp {
        ConsumedTemp {
            reg: self,
            _holds: Vec::new(),
        }
    }
}

struct FunctionFrame {
    alloc_slots: HashMap<Value, usize>,
    alloc_words: usize,
}

impl FunctionFrame {
    fn from_function(func: &FunctionData, bg: &Background) -> Self {
        let mut alloc_slots = HashMap::new();
        let mut alloc_words = 0;
        for (&_bb, node) in func.layout().bbs() {
            for &inst in node.insts().keys() {
                if matches!(bg.get_value(inst).kind(), ValueKind::Alloc(_)) {
                    alloc_slots.insert(inst, alloc_words);
                    alloc_words += 1;
                }
            }
        }
        Self { alloc_slots, alloc_words }
    }

    fn alloc_slot(&self, ptr: Value) -> Option<usize> {
        self.alloc_slots.get(&ptr).cloned()
    }

    fn stack_slot_to_addr(&self, slot: usize) -> AsmValue {
        AsmValue::Offset((slot * 4) as i32, RegName::Stack)
    }

    fn spill_slot_to_addr(&self, slot: usize) -> AsmValue {
        let absolute_slot = self.alloc_words + slot;
        self.stack_slot_to_addr(absolute_slot)
    }
}

fn write_reg_to_location(loc: &RegAddress, src: RegName, frame: &FunctionFrame, asm: &mut Asm) {
    match loc.location.clone() {
        RegLocation::Reg(reg) => {
            if reg != src {
                asm.add_line(AsmLine::Mv(reg, src));
            }
        }
        RegLocation::Stack(slot) => {
            asm.add_line(AsmLine::Store(frame.spill_slot_to_addr(slot), src));
        }
    }
}

fn consume_temp_reg(
    operand: Value,
    user: Value,
    ra: &RegisterAllocator,
    temp_value: &mut HashMap<Value, TempRecord>,
    frame: &FunctionFrame,
    asm: &mut Asm,
) -> Option<ConsumedTemp> {
    let (loc, is_last_use) = {
        let record = temp_value.get_mut(&operand)?;
        record.used_by.remove(&user);
        (record.loc.location.clone(), record.used_by.is_empty())
    };

    let mut holds = Vec::new();
    let reg = match loc {
        RegLocation::Reg(ref reg) => reg.clone(),
        RegLocation::Stack(slot) => {
            let dest = ra.register(user);
            match dest.location.clone() {
                RegLocation::Reg(reg) => {
                    asm.add_line(AsmLine::Load(reg.clone(), frame.spill_slot_to_addr(slot)));
                    holds.push(dest);
                    reg
                }
                RegLocation::Stack(_) => {
                    panic!("No free register for temporary load");
                }
            }
        }
    };
    if is_last_use {
        holds.push(temp_value.remove(&operand)?.loc);
    }
    Some(ConsumedTemp::new(reg, holds))
}

fn alloc_temp_reg_from_ra(
    value: Value,
    ra: &RegisterAllocator,
) -> Option<RegAddress> {
    let addr = ra.register(value);
    match addr.location {
        RegLocation::Reg(_) => Some(addr),
        RegLocation::Stack(_) => None,
    }
}

fn load_integer_operand(
    operand: Value,
    ra: &RegisterAllocator,
    asm: &mut Asm,
    imm: i32,
) -> ConsumedTemp {
    let temp_addr = alloc_temp_reg_from_ra(operand, ra)
        .expect("No free register for integer operand");
    match temp_addr.location.clone() {
        RegLocation::Reg(reg) => {
            asm.add_line(AsmLine::Li(reg.clone(), imm));
            ConsumedTemp::new(reg, vec![temp_addr])
        }
        RegLocation::Stack(_) => unreachable!(),
    }
}

fn consume_or_load_operand(
    operand: Value,
    user: Value,
    bg: &Background,
    ra: &RegisterAllocator,
    frame: &FunctionFrame,
    temp_value: &mut HashMap<Value, TempRecord>,
    asm: &mut Asm,
) -> ConsumedTemp {
    match bg.get_value(operand).kind() {
        ValueKind::Integer(c) => load_integer_operand(operand, ra, asm, c.value()),
        _ => consume_temp_reg(operand, user, ra, temp_value, frame, asm).unwrap(),
    }
}

fn store_to_asm(
    store: &koopa::ir::values::Store,
    bg: &Background,
    ra: &RegisterAllocator,
    frame: &FunctionFrame,
    temp_value: &mut HashMap<Value, TempRecord>,
    asm: &mut Asm,
    user: Value,
) {
    let dest_ptr = store.dest();
    let slot = frame.alloc_slot(dest_ptr).expect("store destination should be local alloc");
    let src = store.value();
    match bg.get_value(src).kind() {
        ValueKind::Integer(c) => {
            let temp_addr = alloc_temp_reg_from_ra(src, ra).expect("No free register for store immediate");
            let temp_reg = match temp_addr.location.clone() {
                RegLocation::Reg(reg) => reg,
                RegLocation::Stack(_) => unreachable!(),
            };
            asm.add_line(AsmLine::Li(temp_reg.clone(), c.value()));
            asm.add_line(AsmLine::Store(frame.stack_slot_to_addr(slot), temp_reg));
        }
        _ => {
            let src_reg = consume_temp_reg(src, user, ra, temp_value, frame, asm)
                .expect("store source should be available")
                .reg();
            asm.add_line(AsmLine::Store(frame.stack_slot_to_addr(slot), src_reg));
        }
    }
}

fn load_to_asm(
    value: Value,
    load: &koopa::ir::values::Load,
    _bg: &Background,
    ra: &RegisterAllocator,
    frame: &FunctionFrame,
    asm: &mut Asm,
) -> RegAddress {
    let src_ptr = load.src();
    let slot = frame.alloc_slot(src_ptr).expect("load source should be local alloc");
    let dest = ra.register(value);
    match dest.location.clone() {
        RegLocation::Reg(reg) => {
            asm.add_line(AsmLine::Load(reg, frame.stack_slot_to_addr(slot)));
        }
        RegLocation::Stack(_) => {
            let temp_addr = alloc_temp_reg_from_ra(src_ptr, ra).expect("No free register for load");
            let temp_reg = match temp_addr.location.clone() {
                RegLocation::Reg(reg) => reg,
                RegLocation::Stack(_) => unreachable!(),
            };
            asm.add_line(AsmLine::Load(temp_reg.clone(), frame.stack_slot_to_addr(slot)));
            write_reg_to_location(&dest, temp_reg, frame, asm);
        }
    }
    dest
}

fn binary_to_asm(
    value: Value,
    binary: &Binary,
    bg: &Background,
    ra: &RegisterAllocator,
    frame: &FunctionFrame,
    asm: &mut Asm,
    temp_value: &mut HashMap<Value, TempRecord>,
) -> RegAddress {
    let dest = ra.register(value);
    let lhs = consume_or_load_operand(binary.lhs(), value, bg, ra, frame, temp_value, asm);
    let rhs = consume_or_load_operand(binary.rhs(), value, bg, ra, frame, temp_value, asm);
    let result_reg = match dest.location {
        RegLocation::Reg(ref reg) => reg.clone(),
        RegLocation::Stack(_) => lhs.reg(),
    };

    match binary.op() {
        BinaryOp::Add => asm.add_line(AsmLine::Add(result_reg.clone(), lhs.reg(), rhs.reg())),
        BinaryOp::Sub => asm.add_line(AsmLine::Sub(result_reg.clone(), lhs.reg(), rhs.reg())),
        BinaryOp::Mul => asm.add_line(AsmLine::Mul(result_reg.clone(), lhs.reg(), rhs.reg())),
        BinaryOp::Div => asm.add_line(AsmLine::Div(result_reg.clone(), lhs.reg(), rhs.reg())),
        BinaryOp::Mod => asm.add_line(AsmLine::Rem(result_reg.clone(), lhs.reg(), rhs.reg())),
        BinaryOp::Eq => {
            asm.add_line(AsmLine::Xor(result_reg.clone(), lhs.reg(), rhs.reg()));
            asm.add_line(AsmLine::Seqz(result_reg.clone(), result_reg.clone()));
        }
        BinaryOp::NotEq => {
            asm.add_line(AsmLine::Xor(result_reg.clone(), lhs.reg(), rhs.reg()));
            asm.add_line(AsmLine::Snez(result_reg.clone(), result_reg.clone()));
        }
        BinaryOp::Le => {
            asm.add_line(AsmLine::Slt(result_reg.clone(), rhs.reg(), lhs.reg()));
            asm.add_line(AsmLine::Xori(result_reg.clone(), result_reg.clone(), 1));
        }
        BinaryOp::Lt => asm.add_line(AsmLine::Slt(result_reg.clone(), lhs.reg(), rhs.reg())),
        BinaryOp::Ge => {
            asm.add_line(AsmLine::Slt(result_reg.clone(), lhs.reg(), rhs.reg()));
            asm.add_line(AsmLine::Xori(result_reg.clone(), result_reg.clone(), 1));
        }
        BinaryOp::Gt => asm.add_line(AsmLine::Slt(result_reg.clone(), rhs.reg(), lhs.reg())),
        BinaryOp::And => asm.add_line(AsmLine::And(result_reg.clone(), lhs.reg(), rhs.reg())),
        BinaryOp::Or => asm.add_line(AsmLine::Or(result_reg.clone(), lhs.reg(), rhs.reg())),
        _ => unimplemented!("{}", format!("Unsupported binary operation: {:?}", binary.op())),
    }

    write_reg_to_location(&dest, result_reg, frame, asm);
    dest
}

impl GenerateAsm for &FunctionData {
    fn to_asm(&self, bg: &Background) -> Asm {
        let bg = &bg.with_dfg(&self.dfg());
        let mut result = Asm::new();
        let mut context = Asm::new();
        let mut temp_value = HashMap::new();
        let frame = FunctionFrame::from_function(self, bg);
        let name = name_to_symbol(self.name());
        result.add_text("\t.text".to_string());
        result.add_text(format!("\t.globl {}", name));
        result.add_text(format!("{}:", name));
        let ra = RegisterAllocator::new();
        for (&_bb, node) in self.layout().bbs() {
            let insts = node.insts().keys();
            for &inst in insts {
                let ret = inst_to_asm(inst, &bg, &ra, &frame, &mut context, &mut temp_value);
                if let Some(ret) = ret {
                    temp_value.insert(inst, TempRecord::from_value(inst, &bg, ret));
                }
            }
        }
        let stack_words = frame.alloc_words + ra.max_stack_size();
        if stack_words > 0 {
            result.add_text(format!("\taddi sp, sp, -{}", stack_words * 4));
            result.add_asm(context);
            result.add_text(format!("\taddi sp, sp, {}", stack_words * 4));
        } else {
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
    frame: &FunctionFrame,
    asm: &mut Asm,
    temp_value: &mut HashMap<Value, TempRecord>,
) -> Option<RegAddress> {
    let inst = bg.get_value(value);
    match inst.kind() {
        ValueKind::Return(r) => {
            if let Some(ret_value) = r.value() {
                match bg.get_value(ret_value).kind() {
                    ValueKind::Integer(c) => {
                        asm.add_line(AsmLine::Li(RegName::Ret, c.value()));
                    }
                    _ => {
                        let ret = consume_temp_reg(ret_value, value, ra, temp_value, frame, asm)
                            .expect("return value should be available");
                        asm.add_line(AsmLine::Mv(RegName::Ret, ret.reg()));
                    }
                }
            }
            None
        }
        ValueKind::Alloc(_) => None,
        ValueKind::Store(store) => {
            store_to_asm(store, bg, ra, frame, temp_value, asm, value);
            None
        }
        ValueKind::Load(load) => Some(load_to_asm(value, load, bg, ra, frame, asm)),
        ValueKind::Integer(_) => {
            panic!("Integer value should not be directly used as an instruction");
        }
        ValueKind::Binary(binary) => Some(binary_to_asm(value, binary, bg, ra, frame, asm, temp_value)),
        ValueKind::Branch(br) => {
            let cond = consume_or_load_operand(br.cond(), value, bg, ra, frame, temp_value, asm);
            unimplemented!();
        }
        _ => panic!("Unsupported instruction: {:?}", inst.kind()),
    }
}