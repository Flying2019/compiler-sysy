use crate::riscv::{AsmLine, AsmValue, RegAddress, RegLocation, RegName, RegisterAllocator};
use core::panic;
use koopa::ir::dfg::DataFlowGraph;
use koopa::ir::entities::{BasicBlockData, ValueData};
use koopa::ir::types::TypeKind;
use koopa::ir::values::{Binary, BinaryOp, Store};
use koopa::ir::{BasicBlock, Function, Value, ValueKind};
use koopa::ir::{FunctionData, Program};
use std::collections::{BTreeSet, HashMap, HashSet};

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
        Self {
            content: Vec::new(),
        }
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
            } else {
                result.push(line.clone());
            }
        }
        self.content = result;
    }

    pub fn to_string(&self) -> String {
        let rendered = self
            .content
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        if rendered.is_empty() {
            rendered
        } else {
            format!("{}\n", rendered)
        }
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
    where
        'a: 'b,
    {
        Background {
            prog: Some(prog),
            ..self.clone()
        }
    }

    pub fn with_dfg<'b>(&self, dfg: &'b DataFlowGraph) -> Background<'b>
    where
        'a: 'b,
    {
        Background {
            dfg: Some(dfg),
            ..self.clone()
        }
    }

    pub fn with_frame<'b>(&self, frame: &'b FunctionFrame) -> Background<'b>
    where
        'a: 'b,
    {
        Background {
            frame: Some(frame),
            ..self.clone()
        }
    }

    pub fn with_ra<'b>(&self, ra: &'b RegisterAllocator) -> Background<'b>
    where
        'a: 'b,
    {
        Background {
            ra: Some(ra),
            ..self.clone()
        }
    }

    pub fn get_value(&self, value: Value) -> &ValueData {
        self.dfg.unwrap().value(value)
    }

    pub fn get_func(&self, func: Function) -> &FunctionData {
        self.prog.unwrap().func(func)
    }

    pub fn get_bb_data(&self, bb: BasicBlock) -> &BasicBlockData {
        self.dfg.unwrap().bb(bb)
    }

    pub fn frame(&self) -> &FunctionFrame {
        self.frame.unwrap()
    }

    pub fn ra(&self) -> &RegisterAllocator {
        self.ra.unwrap()
    }

    pub fn get_glob(&self, value: Value) -> Option<ValueData> {
        self.prog.unwrap().borrow_values().get(&value).cloned()
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
    param_value: HashMap<Value, RegLocation>,
    branch: HashMap<BasicBlock, usize>,
}

impl AsmContext {
    pub fn new() -> Self {
        Self {
            asm: Asm::new(),
            temp_value: HashMap::new(),
            param_value: HashMap::new(),
            branch: HashMap::new(),
        }
    }

    pub fn asm_mut(&mut self) -> &mut Asm {
        &mut self.asm
    }

    pub fn asm_add_line(&mut self, line: AsmLine) {
        self.asm.add_line(line);
    }

    pub fn register_param(&mut self, value: Value, id: usize) {
        if id < 8 {
            self.param_value
                .insert(value, RegLocation::Reg(RegName::Param(id as usize)));
        } else {
            self.param_value
                .insert(value, RegLocation::ParamStack((id - 8) as usize));
        }
    }

    pub fn get_param(&self, value: &Value) -> Option<&RegLocation> {
        self.param_value.get(value)
    }

    pub fn get_temp_value(&self, value: &Value) -> Option<&TempRecord> {
        self.temp_value.get(value)
    }

    pub fn get_temp_value_mut(&mut self, value: &Value) -> Option<&mut TempRecord> {
        self.temp_value.get_mut(value)
    }

    pub fn insert_temp_record(&mut self, value: Value, record: TempRecord) {
        // println!("Insert temp record for value {:?} at location {:?}", value, record.loc.location);
        self.temp_value.insert(value, record);
    }

    pub fn remove_temp_record(&mut self, value: &Value) -> Option<TempRecord> {
        // println!("Remove temp record for value {:?}", value);
        self.temp_value.remove(value)
    }

    pub fn get_branch_id(&mut self, bb: BasicBlock) -> usize {
        if let Some(name) = self.branch.get(&bb) {
            name.clone()
        } else {
            let id = self.branch.len();
            self.branch.insert(bb, id);
            id
        }
    }

    pub fn clear(&mut self) {
        self.asm = Asm::new();
        self.param_value.clear();
    }
}

pub fn program_to_asm(progrma: &Program, bg: &Background) -> Asm {
    let mut result = Asm::new();
    let mut context = AsmContext::new();
    for &glob_var in progrma.borrow_values().keys() {
        let var_data = progrma.borrow_value(glob_var);
        if let Some(name) = var_data.clone().name() {
            result.add_line(AsmLine::DirData);
            result.add_line(AsmLine::Global(name_to_symbol(name)));
            result.add_line(AsmLine::GlobName(name_to_symbol(name)));
            match var_data.kind() {
                ValueKind::GlobalAlloc(glob_alloc) => {
                    let init_value = glob_alloc.init();
                    emit_global_init(init_value, progrma, &mut result);
                }
                _ => unreachable!(),
            }
        }
    }
    for &func in progrma.func_layout() {
        context.clear();
        result.add_asm(function_to_asm(
            &progrma.func(func),
            &bg.with_prog(&progrma),
            &mut context,
        ));
    }
    result
}

fn alloc_size_words(value: &ValueData) -> usize {
    match value.ty().kind() {
        TypeKind::Pointer(base) => base.size() / 4,
        _ => panic!(
            "alloc/globalalloc should have pointer type, found {:?}",
            value.ty()
        ),
    }
}

fn emit_global_init(value: Value, program: &Program, asm: &mut Asm) {
    let value_data = program.borrow_value(value);
    match value_data.kind() {
        ValueKind::ZeroInit(_) => asm.add_line(AsmLine::DirZero(value_data.ty().size())),
        ValueKind::Integer(i) => asm.add_line(AsmLine::DirWord(i.value())),
        ValueKind::Aggregate(aggregate) => {
            for &elem in aggregate.elems() {
                emit_global_init(elem, program, asm);
            }
        }
        _ => unimplemented!(
            "Unsupported global variable initializer: {:?}",
            value_data.kind()
        ),
    }
}

pub struct TempRecord {
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

/// A function frame that manages the stack layout for a function, including the mapping from local allocations to stack slots and the total number of stack words needed for the function.
pub struct FunctionFrame {
    local_slots: HashMap<Value, usize>,
    local_words: usize,
    arg_words: usize,
    caller_save_words: usize,
    save_ra: bool,
    save_fp: bool,
}

// Lowering scratch registers are bounded to t0-t3. SSA values spill to stack,
// so only these caller-saved temporaries ever need save slots across calls.
const CALLER_SAVE_WORDS: usize = 4; // t0-t3
const IMM12_MIN: i32 = -2048;
const IMM12_MAX: i32 = 2047;

impl FunctionFrame {
    fn from_function(func: &FunctionData, bg: &Background) -> Self {
        let mut local_slots = HashMap::new();
        let mut local_words = 0;
        let mut arg_words = 0;
        let mut has_call = false;
        for (&_bb, node) in func.layout().bbs() {
            for &inst in node.insts().keys() {
                if matches!(bg.get_value(inst).kind(), ValueKind::Alloc(_)) {
                    local_slots.insert(inst, local_words);
                    local_words += alloc_size_words(bg.get_value(inst));
                }
            }
        }

        // Pre-scan control-flow instructions to reserve stack slots for
        // all target block parameters used by jump/branch argument passing.
        for (&bb, _node) in func.layout().bbs() {
            for &param in bg.get_bb_data(bb).params() {
                local_slots.insert(param, local_words);
                local_words += 1;
            }
        }

        for (&_bb, node) in func.layout().bbs() {
            for &inst in node.insts().keys() {
                match bg.get_value(inst).kind() {
                    ValueKind::Call(call) => {
                        let arg_count = call.args().len();
                        arg_words = arg_words.max((arg_count as isize - 8).max(0) as usize);
                        has_call = true;
                    }
                    _ => continue,
                };
            }
        }

        Self {
            local_slots,
            local_words,
            arg_words,
            caller_save_words: if has_call { CALLER_SAVE_WORDS } else { 0 },
            save_ra: has_call,
            save_fp: func.params().len() > 8,
        }
    }

    /*
       |-----------------------------------|
       |  Needed for spilled temps         | <- spill words (allocated by register allocator)
       |-----------------------------------|
       |  Local allocs and block params    | <- local words
       |-----------------------------------|
       |  saved ra (if needed)             | <-+
       |-----------------------------------|   |
       |  caller-saved regs (if needed)    |   +- reserve words
       |-----------------------------------|   |
       |  Arguments (if needed)            | <-+
       |-----------------------------------|
       |                                   | <- sp
    */

    fn reserve_words(&self) -> usize {
        self.arg_words
            + self.caller_save_words
            + usize::from(self.save_ra)
            + usize::from(self.save_fp)
    }

    fn local_slot(&self, value: Value) -> Option<usize> {
        Some(self.local_slots.get(&value).cloned()? + self.reserve_words())
    }

    fn stack_slot_to_addr(&self, slot: usize) -> AsmValue {
        AsmValue::Offset((slot * 4) as i32, RegName::Stack)
    }

    fn spill_slot_to_addr(&self, slot: usize) -> AsmValue {
        let absolute_slot = self.local_words + self.reserve_words() + slot;
        self.stack_slot_to_addr(absolute_slot)
    }

    fn arg_to_addr(&self, slot: usize) -> AsmValue {
        assert!(slot < self.arg_words);
        self.stack_slot_to_addr(slot)
    }

    fn caller_save_to_addr(&self, slot: usize) -> AsmValue {
        assert!(slot < self.caller_save_words);
        self.stack_slot_to_addr(self.arg_words + slot)
    }

    fn ra_to_addr(&self) -> AsmValue {
        assert!(self.save_ra);
        self.stack_slot_to_addr(self.arg_words + self.caller_save_words)
    }

    fn fp_to_addr(&self) -> AsmValue {
        assert!(self.save_fp);
        self.stack_slot_to_addr(self.arg_words + self.caller_save_words + usize::from(self.save_ra))
    }
}

/// Check if a value can fit in a 12-bit signed immediate
fn fits_imm12(value: i32) -> bool {
    (IMM12_MIN..=IMM12_MAX).contains(&value)
}

/// If the offset is outside the 12-bit signed immediate, use Li to extend.
fn emit_offset_addr_to_reg(
    asm: &mut Asm,
    dest: RegName,
    base: RegName,
    offset: i32,
    scratch: RegName,
) {
    if fits_imm12(offset) {
        asm.add_line(AsmLine::Addi(dest, base, offset));
    } else {
        asm.add_line(AsmLine::Li(scratch.clone(), offset));
        asm.add_line(AsmLine::Add(dest, base, scratch));
    }
}

fn emit_load_from_offset(
    asm: &mut Asm,
    dest: RegName,
    base: RegName,
    offset: i32,
    scratch: RegName,
) {
    if fits_imm12(offset) {
        asm.add_line(AsmLine::Load(dest, AsmValue::Offset(offset, base)));
    } else {
        emit_offset_addr_to_reg(asm, scratch.clone(), base, offset, scratch.clone());
        asm.add_line(AsmLine::Load(dest, AsmValue::Offset(0, scratch)));
    }
}

fn emit_store_to_offset(asm: &mut Asm, base: RegName, offset: i32, src: RegName, scratch: RegName) {
    if fits_imm12(offset) {
        asm.add_line(AsmLine::Store(AsmValue::Offset(offset, base), src));
    } else {
        emit_offset_addr_to_reg(asm, scratch.clone(), base, offset, scratch.clone());
        asm.add_line(AsmLine::Store(AsmValue::Offset(0, scratch), src));
    }
}

fn write_reg_to_location(
    loc: &RegAddress,
    src: RegName,
    bg: &Background,
    context: &mut AsmContext,
) {
    // Scratch budget: 1 temporary register.
    match loc.location.clone() {
        RegLocation::Reg(reg) => {
            context.asm_add_line(AsmLine::Mv(reg, src));
        }
        RegLocation::Stack(slot) => {
            let (offset, base) = bg.frame().spill_slot_to_addr(slot).expect_offset();
            let scratch = bg.ra().register_temp();
            emit_store_to_offset(context.asm_mut(), base, offset, src, scratch.get_reg_name());
        }
        RegLocation::ParamStack(_) => {
            unreachable!("SSA temporaries should not be stored in param stack slots")
        }
    }
}

fn caller_saved_slot(reg: &RegName) -> Option<usize> {
    match reg {
        RegName::TempT(i) if *i < 7 => Some(*i),
        _ => None,
    }
}

fn live_caller_saved_regs_across_call(call_inst: Value, context: &AsmContext) -> Vec<RegName> {
    let mut regs = BTreeSet::new();
    for record in context.temp_value.values() {
        if !record.used_by.iter().any(|user| *user != call_inst) {
            continue;
        }
        if let RegLocation::Reg(reg) = record.loc.location.clone() {
            if caller_saved_slot(&reg).is_some() {
                regs.insert(reg);
            }
        }
    }
    regs.into_iter().collect()
}

fn save_caller_saved_regs(regs: &[RegName], bg: &Background, context: &mut AsmContext) {
    // Scratch budget: 1 temporary register per iteration.
    for reg in regs {
        let slot = caller_saved_slot(reg).expect("caller-saved register should have stack slot");
        let (offset, base) = bg.frame().caller_save_to_addr(slot).expect_offset();
        let scratch = bg.ra().register_temp();
        emit_store_to_offset(
            context.asm_mut(),
            base,
            offset,
            reg.clone(),
            scratch.get_reg_name(),
        );
    }
}

fn restore_caller_saved_regs(regs: &[RegName], bg: &Background, context: &mut AsmContext) {
    // Scratch budget: 1 temporary register per iteration.
    for reg in regs.iter().rev() {
        let slot = caller_saved_slot(reg).expect("caller-saved register should have stack slot");
        let (offset, base) = bg.frame().caller_save_to_addr(slot).expect_offset();
        let scratch = bg.ra().register_temp();
        emit_load_from_offset(
            context.asm_mut(),
            reg.clone(),
            base,
            offset,
            scratch.get_reg_name(),
        );
    }
}

fn new_temp_reg(loc: RegLocation, bg: &Background, context: &mut AsmContext) -> RegAddress {
    // Scratch budget: 2 temporary registers.
    // One is the returned register, and loading from stack may need one helper scratch.
    let dest_addr = bg.ra().register_temp();
    let dest_reg: RegName = dest_addr.get_reg_name();
    match loc {
        RegLocation::Reg(reg) => context.asm_add_line(AsmLine::Mv(dest_reg.clone(), reg)),
        RegLocation::Stack(slot) => {
            let (offset, base) = bg.frame().spill_slot_to_addr(slot).expect_offset();
            let scratch = bg.ra().register_temp();
            emit_load_from_offset(
                context.asm_mut(),
                dest_reg.clone(),
                base,
                offset,
                scratch.get_reg_name(),
            );
        }
        RegLocation::ParamStack(slot) => {
            let scratch = bg.ra().register_temp();
            emit_load_from_offset(
                context.asm_mut(),
                dest_reg.clone(),
                RegName::TempS(0),
                (slot * 4) as i32,
                scratch.get_reg_name(),
            );
        }
    };
    dest_addr
}

fn load_temp_reg(src: Value, dest: Value, bg: &Background, context: &mut AsmContext) -> RegAddress {
    // Scratch budget: 2 temporary registers via new_temp_reg.
    let (loc, is_last_use) = {
        if let Some(loc) = context.get_param(&src) {
            (loc.clone(), false)
        } else {
            let record = context
                .get_temp_value_mut(&src)
                .expect(&format!("temporary value should be available: {:?}", src));
            record.used_by.remove(&dest);
            (record.loc.location.clone(), record.used_by.is_empty())
        }
    };
    let reg = new_temp_reg(loc, bg, context);
    if is_last_use {
        context
            .remove_temp_record(&src)
            .expect("temporary value should be available");
    }
    reg
}

fn peek_temp_reg(src: Value, bg: &Background, context: &mut AsmContext) -> RegAddress {
    // Scratch budget: 2 temporary registers via new_temp_reg.
    if let Some(loc) = context.get_param(&src) {
        new_temp_reg(loc.clone(), bg, context)
    } else {
        let reg_rec = context
            .get_temp_value(&src)
            .expect("temporary value should be available");
        new_temp_reg(reg_rec.loc.location.clone(), bg, context)
    }
}

fn release_temp_if_unused(value: Value, context: &mut AsmContext) -> Option<RegAddress> {
    if context
        .get_temp_value(&value)
        .is_some_and(|record| record.used_by.is_empty())
    {
        context.remove_temp_record(&value).map(|record| record.loc)
    } else {
        None
    }
}

fn mark_temp_used_by(value: Value, user: Value, context: &mut AsmContext) {
    if let Some(record) = context.get_temp_value_mut(&value) {
        record.used_by.remove(&user);
    }
}

/// Load an operand into a register, returning the register and the temporary values it holds (if any).
fn load_operand_temp(
    src: Value,
    user: Value,
    bg: &Background,
    context: &mut AsmContext,
) -> RegAddress {
    // Scratch budget: 2 temporary registers.
    // Integer immediates take 1, non-immediates defer to load_temp_reg.
    match bg.get_value(src).kind() {
        ValueKind::Integer(c) => {
            let temp_reg = bg.ra().register_temp();
            context.asm_add_line(AsmLine::Li(temp_reg.get_reg_name(), c.value()));
            temp_reg
        }
        _ => load_temp_reg(src, user, bg, context),
    }
}

fn emit_ptr_value(
    ptr: Value,
    user: Value,
    bg: &Background,
    context: &mut AsmContext,
) -> RegAddress {
    // Scratch budget: 2 temporary registers.
    // Loading a local alloc address may need one returned register plus one offset scratch.
    if let Some(value_data) = bg.get_glob(ptr) {
        let name = value_data.name().clone().unwrap();
        let temp_addr = bg.ra().register_temp();
        let temp_reg = temp_addr.get_reg_name();
        context.asm_add_line(AsmLine::La(temp_reg, name_to_symbol(&name)));
        temp_addr
    } else if matches!(bg.get_value(ptr).kind(), ValueKind::Alloc(_)) {
        let slot = bg
            .frame()
            .local_slot(ptr)
            .expect("local alloc should have stack slot");
        let temp_addr = bg.ra().register_temp();
        let temp_reg = temp_addr.get_reg_name();
        let scratch = bg.ra().register_temp();
        emit_offset_addr_to_reg(
            context.asm_mut(),
            temp_reg.clone(),
            RegName::Stack,
            (slot * 4) as i32,
            scratch.get_reg_name(),
        );
        temp_addr
    } else {
        load_temp_reg(ptr, user, bg, context)
    }
}

/// Store the value of a store instruction to its destination, which should be a local allocation. The source value can be an immediate integer or a temporary value.
fn store_to_asm(store: &Store, bg: &Background, context: &mut AsmContext, user: Value) {
    // Scratch budget: 3 temporary registers.
    // Source materialization keeps 1 live register while pointer lowering may use up to 2 more.
    let dest_ptr = store.dest();
    let src = store.value();
    let temp_addr = match bg.get_value(src).kind() {
        ValueKind::Integer(c) => {
            let temp_addr = bg.ra().register_temp();
            let temp_reg = temp_addr.get_reg_name();
            context.asm_add_line(AsmLine::Li(temp_reg.clone(), c.value()));
            temp_addr
        }
        _ => load_temp_reg(src, user, bg, context),
    };
    let dest_addr = emit_ptr_value(dest_ptr, user, bg, context);
    context.asm_add_line(AsmLine::Store(
        AsmValue::Offset(0, dest_addr.get_reg_name()),
        temp_addr.get_reg_name(),
    ));
}

/// Load the value of a load instruction into its destination register, which can be either a physical register or a spill slot. The source pointer should be a local allocation.
fn load_to_asm(value: Value, src: Value, bg: &Background, context: &mut AsmContext) -> RegAddress {
    // Scratch budget: 3 temporary registers.
    // Pointer lowering can use 2, and spilling the loaded result may need 1 extra temporary.
    let ra = bg.ra();
    let temp_ptr = emit_ptr_value(src, value, bg, context);
    let temp_src = AsmValue::Offset(0, temp_ptr.get_reg_name());
    let dest = ra.register(value);
    match dest.location.clone() {
        RegLocation::Reg(reg) => {
            context.asm_add_line(AsmLine::Load(reg, temp_src));
        }
        RegLocation::Stack(_) => {
            let temp_reg = ra.register_temp();
            context.asm_add_line(AsmLine::Load(temp_reg.get_reg_name(), temp_src));
            write_reg_to_location(&dest, temp_reg.get_reg_name(), bg, context);
        }
        RegLocation::ParamStack(_) => {
            unreachable!("register allocator does not assign param stack slots")
        }
    }
    dest
}

fn pointer_step_size(value: Value, bg: &Background) -> i32 {
    match bg.get_value(value).ty().kind() {
        TypeKind::Pointer(base) => base.size() as i32,
        _ => panic!("pointer calculation result should have pointer type"),
    }
}

fn ptr_offset_to_asm(
    value: Value,
    src: Value,
    index: Value,
    bg: &Background,
    context: &mut AsmContext,
) -> RegAddress {
    // Scratch budget: 3 temporary registers.
    // Base pointer, index value, and the scale register can be live together.
    let dest = bg.ra().register(value);
    let base_ptr = emit_ptr_value(src, value, bg, context);
    let index_reg = load_operand_temp(index, value, bg, context);
    let result_reg = match dest.location.clone() {
        RegLocation::Reg(reg) => reg,
        RegLocation::Stack(_) => base_ptr.get_reg_name(),
        RegLocation::ParamStack(_) => {
            unreachable!("register allocator does not assign param stack slots")
        }
    };
    let scale_reg = bg.ra().register_temp();
    let step = pointer_step_size(value, bg);
    context.asm_add_line(AsmLine::Li(scale_reg.get_reg_name(), step));
    context.asm_add_line(AsmLine::Mul(
        scale_reg.get_reg_name(),
        index_reg.get_reg_name(),
        scale_reg.get_reg_name(),
    ));
    context.asm_add_line(AsmLine::Add(
        result_reg.clone(),
        base_ptr.get_reg_name(),
        scale_reg.get_reg_name(),
    ));
    write_reg_to_location(&dest, result_reg, bg, context);
    dest
}

fn binary_to_asm(
    value: Value,
    binary: &Binary,
    bg: &Background,
    context: &mut AsmContext,
) -> RegAddress {
    // Scratch budget: 2 temporary registers for lhs/rhs.
    let dest = bg.ra().register(value);
    let lhs = load_operand_temp(binary.lhs(), value, bg, context);
    let rhs = load_operand_temp(binary.rhs(), value, bg, context);
    let result_reg = match dest.location {
        RegLocation::Reg(ref reg) => reg.clone(),
        RegLocation::Stack(_) => lhs.get_reg_name(),
        RegLocation::ParamStack(_) => {
            unreachable!("register allocator does not assign param stack slots")
        }
    };

    match binary.op() {
        BinaryOp::Add => context.asm_add_line(AsmLine::Add(
            result_reg.clone(),
            lhs.get_reg_name(),
            rhs.get_reg_name(),
        )),
        BinaryOp::Sub => context.asm_add_line(AsmLine::Sub(
            result_reg.clone(),
            lhs.get_reg_name(),
            rhs.get_reg_name(),
        )),
        BinaryOp::Mul => context.asm_add_line(AsmLine::Mul(
            result_reg.clone(),
            lhs.get_reg_name(),
            rhs.get_reg_name(),
        )),
        BinaryOp::Div => context.asm_add_line(AsmLine::Div(
            result_reg.clone(),
            lhs.get_reg_name(),
            rhs.get_reg_name(),
        )),
        BinaryOp::Mod => context.asm_add_line(AsmLine::Rem(
            result_reg.clone(),
            lhs.get_reg_name(),
            rhs.get_reg_name(),
        )),
        BinaryOp::Eq => {
            context.asm_add_line(AsmLine::Xor(
                result_reg.clone(),
                lhs.get_reg_name(),
                rhs.get_reg_name(),
            ));
            context.asm_add_line(AsmLine::Seqz(result_reg.clone(), result_reg.clone()));
        }
        BinaryOp::NotEq => {
            context.asm_add_line(AsmLine::Xor(
                result_reg.clone(),
                lhs.get_reg_name(),
                rhs.get_reg_name(),
            ));
            context.asm_add_line(AsmLine::Snez(result_reg.clone(), result_reg.clone()));
        }
        BinaryOp::Le => {
            context.asm_add_line(AsmLine::Slt(
                result_reg.clone(),
                rhs.get_reg_name(),
                lhs.get_reg_name(),
            ));
            context.asm_add_line(AsmLine::Xori(result_reg.clone(), result_reg.clone(), 1));
        }
        BinaryOp::Lt => context.asm_add_line(AsmLine::Slt(
            result_reg.clone(),
            lhs.get_reg_name(),
            rhs.get_reg_name(),
        )),
        BinaryOp::Ge => {
            context.asm_add_line(AsmLine::Slt(
                result_reg.clone(),
                lhs.get_reg_name(),
                rhs.get_reg_name(),
            ));
            context.asm_add_line(AsmLine::Xori(result_reg.clone(), result_reg.clone(), 1));
        }
        BinaryOp::Gt => context.asm_add_line(AsmLine::Slt(
            result_reg.clone(),
            rhs.get_reg_name(),
            lhs.get_reg_name(),
        )),
        BinaryOp::And => context.asm_add_line(AsmLine::And(
            result_reg.clone(),
            lhs.get_reg_name(),
            rhs.get_reg_name(),
        )),
        BinaryOp::Or => context.asm_add_line(AsmLine::Or(
            result_reg.clone(),
            lhs.get_reg_name(),
            rhs.get_reg_name(),
        )),
        _ => unimplemented!(
            "{}",
            format!("Unsupported binary operation: {:?}", binary.op())
        ),
    }

    write_reg_to_location(&dest, result_reg, bg, context);
    dest
}

fn load_block_param(param: Value, bg: &Background, context: &mut AsmContext) -> RegAddress {
    // Scratch budget: 3 temporary registers.
    // Loading into a spilled SSA slot keeps the loaded value live while write_reg_to_location
    // uses one more helper scratch.
    let frame = bg.frame();
    let slot = frame
        .local_slot(param)
        .expect("block parameter should have stack slot");
    let dest = bg.ra().register(param);
    match dest.location.clone() {
        RegLocation::Reg(reg) => {
            let scratch = bg.ra().register_temp();
            emit_load_from_offset(
                context.asm_mut(),
                reg,
                RegName::Stack,
                (slot * 4) as i32,
                scratch.get_reg_name(),
            );
        }
        RegLocation::Stack(_) => {
            let temp_addr = bg.ra().register_temp();
            let temp_reg = temp_addr.get_reg_name();
            let scratch = bg.ra().register_temp();
            emit_load_from_offset(
                context.asm_mut(),
                temp_reg.clone(),
                RegName::Stack,
                (slot * 4) as i32,
                scratch.get_reg_name(),
            );
            write_reg_to_location(&dest, temp_reg, bg, context);
        }
        RegLocation::ParamStack(_) => {
            unreachable!("register allocator does not assign param stack slots")
        }
    }
    dest
}

fn store_block_args(
    target: BasicBlock,
    args: &[Value],
    bg: &Background,
    context: &mut AsmContext,
) -> Vec<Value> {
    // Scratch budget: 2 temporary registers per argument.
    // One holds the value to store and one extends large stack offsets when needed.
    let params = bg.get_bb_data(target).params();
    if params.len() != args.len() {
        panic!(
            "Argument count does not match for jump/branch target {:?}",
            target
        );
    }
    let mut block_args = Vec::new();
    for pos in 0..params.len() {
        let arg = args[pos];
        let param = params[pos];
        let slot = bg
            .frame()
            .local_slot(param)
            .expect("block parameter should have stack slot");
        match bg.get_value(arg).kind() {
            ValueKind::Integer(c) => {
                let temp_addr = bg.ra().register_temp();
                let temp_reg = temp_addr.get_reg_name();
                context.asm_add_line(AsmLine::Li(temp_reg.clone(), c.value()));
                let scratch = bg.ra().register_temp();
                emit_store_to_offset(
                    context.asm_mut(),
                    RegName::Stack,
                    (slot * 4) as i32,
                    temp_reg,
                    scratch.get_reg_name(),
                );
            }
            _ => {
                let temp_reg = peek_temp_reg(arg, bg, context);
                let scratch = bg.ra().register_temp();
                emit_store_to_offset(
                    context.asm_mut(),
                    RegName::Stack,
                    (slot * 4) as i32,
                    temp_reg.get_reg_name(),
                    scratch.get_reg_name(),
                );
                block_args.push(arg);
            }
        }
    }
    block_args
}

fn release_block_args(
    block_args: Vec<Value>,
    user: Value,
    context: &mut AsmContext,
) -> Vec<RegAddress> {
    let mut released = Vec::new();
    let mut seen = HashSet::new();
    for arg in block_args {
        if seen.insert(arg) {
            mark_temp_used_by(arg, user, context);
            if let Some(loc) = release_temp_if_unused(arg, context) {
                released.push(loc);
            }
        }
    }
    released
}

fn function_to_asm(func: &FunctionData, bg: &Background, context: &mut AsmContext) -> Asm {
    if func.layout().entry_bb().is_none() {
        // This function is a declaration without a body
        return Asm::new();
    }
    let bg = &bg.with_dfg(&func.dfg());
    let frame = FunctionFrame::from_function(func, bg);
    let ra = RegisterAllocator::new();
    let bg = &bg.with_frame(&frame).with_ra(&ra);
    let mut result = Asm::new();
    let name = name_to_symbol(func.name());
    result.add_line(AsmLine::DirText);
    result.add_line(AsmLine::Global(name.clone()));
    result.add_line(AsmLine::GlobName(name.clone()));
    for (id, &arg) in func.params().iter().enumerate() {
        context.register_param(arg, id);
    }
    for (&bb, node) in func.layout().bbs() {
        let br_id = context.get_branch_id(bb);
        context.asm_add_line(AsmLine::Label(br_id));
        for &param in bg.get_bb_data(bb).params() {
            let loaded = load_block_param(param, bg, context);
            context.insert_temp_record(param, TempRecord::from_value(param, &bg, loaded));
        }
        let insts = node.insts().keys();
        for &inst in insts {
            let ret = inst_to_asm(inst, &bg, context);
            if let Some(ret) = ret {
                let temp_record = TempRecord::from_value(inst, &bg, ret);
                if !temp_record.used_by.is_empty() {
                    context.insert_temp_record(inst, temp_record);
                }
            }
        }
    }
    let stack_words = frame.local_words + frame.reserve_words() + ra.max_stack_size();
    if stack_words > 0 {
        let stack_len = stack_words as i32 * 4;
        // Align stack to 16 bytes for function calls
        let aligned_stack_len = ((stack_len + 15) / 16) * 16;
        emit_offset_addr_to_reg(
            &mut result,
            RegName::Stack,
            RegName::Stack,
            -aligned_stack_len,
            RegName::TempT(0),
        );
        if frame.save_fp {
            let (offset, base) = frame.fp_to_addr().expect_offset();
            emit_store_to_offset(
                &mut result,
                base,
                offset,
                RegName::TempS(0),
                RegName::TempT(0),
            );
            emit_offset_addr_to_reg(
                &mut result,
                RegName::TempS(0),
                RegName::Stack,
                aligned_stack_len,
                RegName::TempT(0),
            );
        }
        let mut replacement = Asm::new();
        if frame.save_fp {
            let (offset, base) = frame.fp_to_addr().expect_offset();
            emit_load_from_offset(
                &mut replacement,
                RegName::TempS(0),
                base,
                offset,
                RegName::TempT(0),
            );
        }
        if frame.save_ra {
            // Save ra if needed
            let (offset, base) = frame.ra_to_addr().expect_offset();
            emit_store_to_offset(
                &mut result,
                base.clone(),
                offset,
                RegName::Ra,
                RegName::TempT(0),
            );
            emit_load_from_offset(
                &mut replacement,
                RegName::Ra,
                base,
                offset,
                RegName::TempT(0),
            );
        }
        emit_offset_addr_to_reg(
            &mut replacement,
            RegName::Stack,
            RegName::Stack,
            aligned_stack_len,
            RegName::TempT(0),
        );
        replacement.add_line(AsmLine::Ret);
        context
            .asm_mut()
            .convert_placehold("return".to_string(), replacement.content);
        result.add_asm(context.asm_mut().clone());
    } else {
        context
            .asm_mut()
            .convert_placehold("return".to_string(), vec![AsmLine::Ret]);
        result.add_asm(context.asm_mut().clone());
    }
    result
}

fn inst_to_asm(value: Value, bg: &Background, context: &mut AsmContext) -> Option<RegAddress> {
    let inst = bg.get_value(value);
    match inst.kind() {
        ValueKind::Return(r) => {
            if let Some(ret_value) = r.value() {
                match bg.get_value(ret_value).kind() {
                    ValueKind::Integer(c) => {
                        context.asm_add_line(AsmLine::Li(RegName::Ret, c.value()));
                    }
                    _ => {
                        let ret = load_temp_reg(ret_value, value, bg, context);
                        context.asm_add_line(AsmLine::Mv(RegName::Ret, ret.get_reg_name()));
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
        ValueKind::GetPtr(ptr) => Some(ptr_offset_to_asm(
            value,
            ptr.src(),
            ptr.index(),
            bg,
            context,
        )),
        ValueKind::GetElemPtr(ptr) => Some(ptr_offset_to_asm(
            value,
            ptr.src(),
            ptr.index(),
            bg,
            context,
        )),
        ValueKind::Integer(_) => {
            panic!("Integer value should not be directly used as an instruction");
        }
        ValueKind::Binary(binary) => Some(binary_to_asm(value, binary, bg, context)),
        ValueKind::Branch(br) => {
            let mut block_args = store_block_args(br.true_bb(), br.true_args(), bg, context);
            block_args.extend(store_block_args(
                br.false_bb(),
                br.false_args(),
                bg,
                context,
            ));
            let cond = load_operand_temp(br.cond(), value, bg, context);
            let _released_block_args = release_block_args(block_args, value, context);
            let then_id = context.get_branch_id(br.true_bb());
            let else_id = context.get_branch_id(br.false_bb());
            context.asm_add_line(AsmLine::Beqz(cond.get_reg_name(), else_id));
            context.asm_add_line(AsmLine::Jump(then_id));
            None
        }
        ValueKind::Jump(jump) => {
            let block_args = store_block_args(jump.target(), jump.args(), bg, context);
            let _released_block_args = release_block_args(block_args, value, context);
            let target_id = context.get_branch_id(jump.target());
            context.asm_add_line(AsmLine::Jump(target_id));
            None
        }
        ValueKind::Call(call) => {
            let frame = bg.frame();
            let func_data = bg.get_func(call.callee());
            let args = call.args();
            let saved_regs = live_caller_saved_regs_across_call(value, context);
            save_caller_saved_regs(&saved_regs, bg, context);
            for (i, &arg) in args.iter().enumerate() {
                let temp_reg = load_operand_temp(arg, value, bg, context);
                if i < 8 {
                    context.asm_add_line(AsmLine::Mv(RegName::Param(i), temp_reg.get_reg_name()));
                } else {
                    let (offset, base) = frame.arg_to_addr(i - 8).expect_offset();
                    let scratch = bg.ra().register_temp();
                    emit_store_to_offset(
                        context.asm_mut(),
                        base,
                        offset,
                        temp_reg.get_reg_name(),
                        scratch.get_reg_name(),
                    );
                }
            }
            context.asm_add_line(AsmLine::Call(name_to_symbol(func_data.name())));
            if inst.ty().is_unit() {
                restore_caller_saved_regs(&saved_regs, bg, context);
                return None;
            }
            let ret = bg.ra().register(value);
            restore_caller_saved_regs(&saved_regs, bg, context);
            write_reg_to_location(&ret, RegName::Ret, bg, context);
            ret.into()
        }
        _ => panic!("Unsupported instruction: {:?}", inst.kind()),
    }
}
