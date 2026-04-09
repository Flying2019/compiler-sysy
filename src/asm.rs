use std::collections::{HashMap, HashSet};
use koopa::ir::dfg::DataFlowGraph;
use koopa::ir::entities::ValueData;
use koopa::ir::values::{Binary, BinaryOp};
use koopa::ir::{BasicBlock, Value, ValueKind};
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
#[derive(Clone)]
pub struct Asm {
    content: Vec<AsmLine>,
}

impl Asm {
    pub fn new() -> Self {
        Self { content: Vec::new() }
    }

    pub fn add_asm(&mut self, func_asm: Asm) {
        self.content.extend(func_asm.content);
    }

    pub fn add_line(&mut self, line: AsmLine) {
        self.content.push(line);
    }

    pub fn convert_placehold(&mut self, placeholder: String, replacement: Vec<AsmLine>) {
        let mut result = Vec::new();
        for line in self.content.iter_mut() {
            if *line == AsmLine::PlaceHold(placeholder.clone()) {
                result.extend(replacement.clone());
            }
            else {
                result.push(line.clone());
            }
        }
        self.content = result;
    }

    pub fn to_string(&self) -> String {
        self.content.iter().map(|line| line.to_string()).collect::<Vec<_>>().join("\n")
    }
}

pub struct Background<'a> {
    prog: Option<&'a Program>,
    dfg: Option<&'a DataFlowGraph>,
    frame: Option<&'a FunctionFrame>,
    ra: Option<&'a RegisterAllocator>,
}

impl<'a> Background<'a> {
    pub fn new() -> Self {
        Self {
            prog: None,
            dfg: None,
            frame: None,
            ra: None,
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

    pub fn with_frame<'b>(&self, frame: &'b FunctionFrame) -> Background<'b>
        where 'a: 'b,
    {
        Background { frame: Some(frame), ..self.clone() }
    }

    pub fn with_ra<'b>(&self, ra: &'b RegisterAllocator) -> Background<'b>
        where 'a: 'b,
    {
        Background { ra: Some(ra), ..self.clone() }
    }

    pub fn get_value(&self, value: Value) -> &ValueData {
        self.dfg.unwrap().value(value)
    }

    pub fn frame(&self) -> &FunctionFrame {
        self.frame.unwrap()
    }

    pub fn ra(&self) -> &RegisterAllocator {
        self.ra.unwrap()
    }
}

impl<'a> Clone for Background<'a> {
    fn clone(&self) -> Self {
        Self {
            prog: self.prog,
            dfg: self.dfg,
            frame: self.frame,
            ra: self.ra,
        }
    }
}

pub struct AsmContext {
    asm: Asm,
    temp_value: HashMap<Value, TempRecord>,
    branch: HashMap<BasicBlock, usize>,
}

impl AsmContext {
    pub fn new() -> Self {
        Self {
            asm: Asm::new(),
            temp_value: HashMap::new(),
            branch: HashMap::new(),
        }
    }

    fn asm_mut(&mut self) -> &mut Asm {
        &mut self.asm
    }

    fn asm_add_line(&mut self, line: AsmLine) {
        self.asm.add_line(line);
    }

    fn temp_value_mut(&mut self) -> &mut HashMap<Value, TempRecord> {
        &mut self.temp_value
    }

    fn insert_temp_record(&mut self, value: Value, record: TempRecord) {
        self.temp_value.insert(value, record);
    }
    
    fn remove_temp_record(&mut self, value: &Value) -> Option<TempRecord> {
        self.temp_value.remove(value)
    }

    fn get_branch_id(&mut self, bb: BasicBlock) -> usize {
        if let Some(name) = self.branch.get(&bb) {
            name.clone()
        } else {
            let id = self.branch.len();
            self.branch.insert(bb, id);
            id
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

pub struct FunctionFrame {
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

fn write_reg_to_location(loc: &RegAddress, src: RegName, frame: &FunctionFrame, context: &mut AsmContext) {
    match loc.location.clone() {
        RegLocation::Reg(reg) => {
            if reg != src {
                context.asm_add_line(AsmLine::Mv(reg, src));
            }
        }
        RegLocation::Stack(slot) => {
            context.asm_add_line(AsmLine::Store(frame.spill_slot_to_addr(slot), src));
        }
    }
}

fn load_temp_reg(
    operand: Value,
    user: Value,
    bg: &Background,
    context: &mut AsmContext,
) -> Option<ConsumedTemp> {
    let (loc, is_last_use) = {
        let temp_value = context.temp_value_mut();
        let record = temp_value.get_mut(&operand)?;
        record.used_by.remove(&user);
        (record.loc.location.clone(), record.used_by.is_empty())
    };

    let mut holds = Vec::new();
    let reg = match loc {
        RegLocation::Reg(ref reg) => reg.clone(),
        RegLocation::Stack(slot) => {
            let dest = bg.ra().register(user);
            match dest.location.clone() {
                RegLocation::Reg(reg) => {
                    context.asm_add_line(AsmLine::Load(reg.clone(), bg.frame().spill_slot_to_addr(slot)));
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
        let released = context.remove_temp_record(&operand)?;
        holds.push(released.loc);
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
    context: &mut AsmContext,
    imm: i32,
) -> ConsumedTemp {
    let temp_addr = alloc_temp_reg_from_ra(operand, ra)
        .expect("No free register for integer operand");
    match temp_addr.location.clone() {
        RegLocation::Reg(reg) => {
            context.asm_add_line(AsmLine::Li(reg.clone(), imm));
            ConsumedTemp::new(reg, vec![temp_addr])
        }
        RegLocation::Stack(_) => unreachable!(),
    }
}

fn load_operand(
    operand: Value,
    user: Value,
    bg: &Background,
    context: &mut AsmContext,
) -> ConsumedTemp {
    match bg.get_value(operand).kind() {
        ValueKind::Integer(c) => load_integer_operand(operand, bg.ra(), context, c.value()),
        _ => load_temp_reg(operand, user, bg, context).unwrap(),
    }
}

fn store_to_asm(
    store: &koopa::ir::values::Store,
    bg: &Background,
    context: &mut AsmContext,
    user: Value,
) {
    let dest_ptr = store.dest();
    let slot = bg.frame().alloc_slot(dest_ptr).expect("store destination should be local alloc");
    let src = store.value();
    match bg.get_value(src).kind() {
        ValueKind::Integer(c) => {
            let temp_addr = alloc_temp_reg_from_ra(src, bg.ra()).expect("No free register for store immediate");
            let temp_reg = match temp_addr.location.clone() {
                RegLocation::Reg(reg) => reg,
                RegLocation::Stack(_) => unreachable!(),
            };
            context.asm_add_line(AsmLine::Li(temp_reg.clone(), c.value()));
            context.asm_add_line(AsmLine::Store(bg.frame().stack_slot_to_addr(slot), temp_reg));
        }
        _ => {
            let src_reg = load_temp_reg(src, user, bg, context)
                .expect("store source should be available")
                .reg();
            context.asm_add_line(AsmLine::Store(bg.frame().stack_slot_to_addr(slot), src_reg));
        }
    }
}

fn load_to_asm(
    value: Value,
    src_ptr: Value,
    bg: &Background,
    context: &mut AsmContext,
) -> RegAddress {
    let slot = bg.frame().alloc_slot(src_ptr).expect("load source should be local alloc");
    let dest = bg.ra().register(value);
    match dest.location.clone() {
        RegLocation::Reg(reg) => {
            context.asm_add_line(AsmLine::Load(reg, bg.frame().stack_slot_to_addr(slot)));
        }
        RegLocation::Stack(_) => {
            let temp_addr = alloc_temp_reg_from_ra(src_ptr, bg.ra()).expect("No free register for load");
            let temp_reg = match temp_addr.location.clone() {
                RegLocation::Reg(reg) => reg,
                RegLocation::Stack(_) => unreachable!(),
            };
            context.asm_add_line(AsmLine::Load(temp_reg.clone(), bg.frame().stack_slot_to_addr(slot)));
            write_reg_to_location(&dest, temp_reg, bg.frame(), context);
        }
    }
    dest
}

fn binary_to_asm(
    value: Value,
    binary: &Binary,
    bg: &Background,
    context: &mut AsmContext,
) -> RegAddress {
    let dest = bg.ra().register(value);
    let lhs = load_operand(binary.lhs(), value, bg, context);
    let rhs = load_operand(binary.rhs(), value, bg, context);
    let result_reg = match dest.location {
        RegLocation::Reg(ref reg) => reg.clone(),
        RegLocation::Stack(_) => lhs.reg(),
    };

    match binary.op() {
        BinaryOp::Add => context.asm_add_line(AsmLine::Add(result_reg.clone(), lhs.reg(), rhs.reg())),
        BinaryOp::Sub => context.asm_add_line(AsmLine::Sub(result_reg.clone(), lhs.reg(), rhs.reg())),
        BinaryOp::Mul => context.asm_add_line(AsmLine::Mul(result_reg.clone(), lhs.reg(), rhs.reg())),
        BinaryOp::Div => context.asm_add_line(AsmLine::Div(result_reg.clone(), lhs.reg(), rhs.reg())),
        BinaryOp::Mod => context.asm_add_line(AsmLine::Rem(result_reg.clone(), lhs.reg(), rhs.reg())),
        BinaryOp::Eq => {
            context.asm_add_line(AsmLine::Xor(result_reg.clone(), lhs.reg(), rhs.reg()));
            context.asm_add_line(AsmLine::Seqz(result_reg.clone(), result_reg.clone()));
        }
        BinaryOp::NotEq => {
            context.asm_add_line(AsmLine::Xor(result_reg.clone(), lhs.reg(), rhs.reg()));
            context.asm_add_line(AsmLine::Snez(result_reg.clone(), result_reg.clone()));
        }
        BinaryOp::Le => {
            context.asm_add_line(AsmLine::Slt(result_reg.clone(), rhs.reg(), lhs.reg()));
            context.asm_add_line(AsmLine::Xori(result_reg.clone(), result_reg.clone(), 1));
        }
        BinaryOp::Lt => context.asm_add_line(AsmLine::Slt(result_reg.clone(), lhs.reg(), rhs.reg())),
        BinaryOp::Ge => {
            context.asm_add_line(AsmLine::Slt(result_reg.clone(), lhs.reg(), rhs.reg()));
            context.asm_add_line(AsmLine::Xori(result_reg.clone(), result_reg.clone(), 1));
        }
        BinaryOp::Gt => context.asm_add_line(AsmLine::Slt(result_reg.clone(), rhs.reg(), lhs.reg())),
        BinaryOp::And => context.asm_add_line(AsmLine::And(result_reg.clone(), lhs.reg(), rhs.reg())),
        BinaryOp::Or => context.asm_add_line(AsmLine::Or(result_reg.clone(), lhs.reg(), rhs.reg())),
        _ => unimplemented!("{}", format!("Unsupported binary operation: {:?}", binary.op())),
    }

    write_reg_to_location(&dest, result_reg, bg.frame(), context);
    dest
}

impl GenerateAsm for &FunctionData {
    fn to_asm(&self, bg: &Background) -> Asm {
        let bg = &bg.with_dfg(&self.dfg());
        let frame = FunctionFrame::from_function(self, bg);
        let ra = RegisterAllocator::new();
        let bg = &bg.with_frame(&frame).with_ra(&ra);
        let mut result = Asm::new();
        let mut context = AsmContext::new();
        let name = name_to_symbol(self.name());
        // result.add_text("\t.text".to_string());
        // result.add_text(format!("\t.globl {}", name));
        // result.add_text(format!("{}:", name));
        result.add_line(AsmLine::Text);
        result.add_line(AsmLine::Global(name.clone()));
        result.add_line(AsmLine::Func(name.clone()));
        for (&bb, node) in self.layout().bbs() {
            let br_id = context.get_branch_id(bb);
            context.asm_add_line(AsmLine::Label(br_id));
            println!("Processing basic block {:?}", bb);
            let insts = node.insts().keys();
            for &inst in insts {
                let ret = inst_to_asm(inst, &bg, &mut context);
                if let Some(ret) = ret {
                    context.insert_temp_record(inst, TempRecord::from_value(inst, &bg, ret));
                }
            }
        }
        let stack_words = frame.alloc_words + ra.max_stack_size();
        if stack_words > 0 {
            let stack_len = stack_words as i32 * 4;
            result.add_line(AsmLine::Addi(RegName::Stack, RegName::Stack, -stack_len));
            context.asm_mut().convert_placehold("return".to_string(), vec![
                AsmLine::Addi(RegName::Stack, RegName::Stack, stack_len),
                AsmLine::Ret
            ]);
            result.add_asm(context.asm_mut().clone());
        } else {
            context.asm_mut().convert_placehold("return".to_string(), vec![AsmLine::Ret]);
            result.add_asm(context.asm_mut().clone());
        }
        result
    }
}

fn inst_to_asm(
    value: Value,
    bg: &Background,
    context: &mut AsmContext,
) -> Option<RegAddress> {
    let inst = bg.get_value(value);
    match inst.kind() {
        ValueKind::Return(r) => {
            if let Some(ret_value) = r.value() {
                match bg.get_value(ret_value).kind() {
                    ValueKind::Integer(c) => {
                        context.asm_add_line(AsmLine::Li(RegName::Ret, c.value()));
                    }
                    _ => {
                        let ret = load_temp_reg(ret_value, value, bg, context)
                            .expect("return value should be available");
                        context.asm_add_line(AsmLine::Mv(RegName::Ret, ret.reg()));
                    }
                }
            }
            context.asm_add_line(AsmLine::PlaceHold("return".to_string()));
            None
        }
        ValueKind::Alloc(_) => None,
        ValueKind::Store(store) => {
            store_to_asm(store, bg, context, value);
            None
        }
        ValueKind::Load(load) => Some(load_to_asm(value, load.src(), bg, context)),
        ValueKind::Integer(_) => {
            panic!("Integer value should not be directly used as an instruction");
        }
        ValueKind::Binary(binary) => Some(binary_to_asm(value, binary, bg, context)),
        ValueKind::Branch(br) => {
            let cond = load_operand(br.cond(), value, bg, context);
            let then_id = context.get_branch_id(br.true_bb());
            let else_id = context.get_branch_id(br.false_bb());
            context.asm_add_line(AsmLine::Beqz(cond.reg(), else_id));
            context.asm_add_line(AsmLine::Jump(then_id));
            None
        }
        ValueKind::Jump(jump) => {
            let target_id = context.get_branch_id(jump.target());
            context.asm_add_line(AsmLine::Jump(target_id));
            None
        }
        _ => panic!("Unsupported instruction: {:?}", inst.kind()),
    }
}