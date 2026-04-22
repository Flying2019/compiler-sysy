use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use koopa::ir::Value;

#[derive(Eq, PartialOrd, PartialEq, Clone, Debug)]
pub enum RegName {
    Ret,
    Param(usize),
    TempT(usize),
    TempS(usize),
    Ra,
    Stack,
}

impl Ord for RegName {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match RegName::partial_cmp(self, other) {
            Some(ordering) => ordering,
            None => self.to_string().cmp(&other.to_string()),
        }
    }
}

impl RegName {
    pub fn to_string(&self) -> String {
        match self {
            RegName::Ret => "a0".to_string(),
            RegName::Param(i) => format!("a{}", i),
            RegName::TempT(i) => format!("t{}", i),
            RegName::TempS(i) => format!("s{}", i),
            RegName::Ra => "ra".to_string(),
            RegName::Stack => "sp".to_string(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum AsmValue {
    Reg(RegName),
    Offset(i32, RegName),
    Const(i32),
}

impl AsmValue {
    pub fn to_string(&self) -> String {
        match self {
            AsmValue::Reg(reg) => reg.to_string(),
            AsmValue::Offset(offset, reg) => format!("{}({})", offset, reg.to_string()),
            AsmValue::Const(c) => c.to_string(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum AsmLine {
    // PlaceHold
    PlaceHold(String),
    // Symbol
    DirData,
    DirText,
    Global(String),
    GlobName(String),
    DirZero(usize),
    DirWord(i32),
    // Control flow
    Label(usize),
    Jump(usize),
    Ret,
    Call(String),
    Beqz(RegName, usize),
    Bnez(RegName, usize),
    // Data movement
    Mv(RegName, RegName),
    Load(RegName, AsmValue),
    Store(AsmValue, RegName),
    Li(RegName, i32),
    La(RegName, String),
    // Arithmetic
    Add(RegName, RegName, RegName),
    Addi(RegName, RegName, i32),
    Sub(RegName, RegName, RegName),
    Slt(RegName, RegName, RegName),
    Sgt(RegName, RegName, RegName),
    Seqz(RegName, RegName),
    Snez(RegName, RegName),
    Xor(RegName, RegName, RegName),
    Xori(RegName, RegName, i32),
    Or(RegName, RegName, RegName),
    Ori(RegName, RegName, i32),
    And(RegName, RegName, RegName),
    Andi(RegName, RegName, i32),
    Sll(RegName, RegName, RegName),
    Srl(RegName, RegName, RegName),
    Sra(RegName, RegName, RegName),
    Mul(RegName, RegName, RegName),
    Div(RegName, RegName, RegName),
    Rem(RegName, RegName, RegName),
}

impl AsmLine {
    pub fn to_string(&self) -> String {
        match self {
            // PlaceHold
            AsmLine::PlaceHold(s) => panic!("PlaceHold should not be converted to string: {}", s),
            // Symbol
            AsmLine::DirText => "\n\t.text".to_string(),
            AsmLine::DirData => "\n\t.data".to_string(),
            AsmLine::Global(name) => format!("\t.global {}", name),
            AsmLine::GlobName(name) => format!("{}:", name),
            AsmLine::DirZero(size) => format!("\t.zero {}", size),
            AsmLine::DirWord(value) => format!("\t.word {}", value),
            // Control flow
            AsmLine::Label(id) => format!("L{}:", id),
            AsmLine::Jump(id) => format!("\tj L{}", id),
            AsmLine::Ret => "\tret".to_string(),
            AsmLine::Call(func) => format!("\tcall {}", func),
            AsmLine::Beqz(reg, id) => format!("\tbeqz {}, L{}", reg.to_string(), id),
            AsmLine::Bnez(reg, id) => format!("\tbnez {}, L{}", reg.to_string(), id),
            // Data movement
            AsmLine::Mv(dest, src) => format!("\tmv {}, {}", dest.to_string(), src.to_string()),
            AsmLine::Load(dest, addr) => format!("\tlw {}, {}", dest.to_string(), addr.to_string()),
            AsmLine::Store(addr, src) => format!("\tsw {}, {}", src.to_string(), addr.to_string()),
            AsmLine::La(dest, label) => format!("\tla {}, {}", dest.to_string(), label),
            AsmLine::Li(dest, imm) => format!("\tli {}, {}", dest.to_string(), imm),
            // Arithmetic
            AsmLine::Add(dest, src1, src2) => format!(
                "\tadd {}, {}, {}",
                dest.to_string(),
                src1.to_string(),
                src2.to_string()
            ),
            AsmLine::Addi(dest, src, imm) => {
                format!("\taddi {}, {}, {}", dest.to_string(), src.to_string(), imm)
            }
            AsmLine::Sub(dest, src1, src2) => format!(
                "\tsub {}, {}, {}",
                dest.to_string(),
                src1.to_string(),
                src2.to_string()
            ),
            AsmLine::Slt(dest, src1, src2) => format!(
                "\tslt {}, {}, {}",
                dest.to_string(),
                src1.to_string(),
                src2.to_string()
            ),
            AsmLine::Sgt(dest, src1, src2) => format!(
                "\tsgt {}, {}, {}",
                dest.to_string(),
                src1.to_string(),
                src2.to_string()
            ),
            AsmLine::Seqz(dest, src) => format!("\tseqz {}, {}", dest.to_string(), src.to_string()),
            AsmLine::Snez(dest, src) => format!("\tsnez {}, {}", dest.to_string(), src.to_string()),
            AsmLine::Xor(dest, src1, src2) => format!(
                "\txor {}, {}, {}",
                dest.to_string(),
                src1.to_string(),
                src2.to_string()
            ),
            AsmLine::Xori(dest, src, imm) => {
                format!("\txori {}, {}, {}", dest.to_string(), src.to_string(), imm)
            }
            AsmLine::Or(dest, src1, src2) => format!(
                "\tor {}, {}, {}",
                dest.to_string(),
                src1.to_string(),
                src2.to_string()
            ),
            AsmLine::Ori(dest, src, imm) => {
                format!("\tori {}, {}, {}", dest.to_string(), src.to_string(), imm)
            }
            AsmLine::And(dest, src1, src2) => format!(
                "\tand {}, {}, {}",
                dest.to_string(),
                src1.to_string(),
                src2.to_string()
            ),
            AsmLine::Andi(dest, src, imm) => {
                format!("\tandi {}, {}, {}", dest.to_string(), src.to_string(), imm)
            }
            AsmLine::Sll(dest, src1, src2) => format!(
                "\tsll {}, {}, {}",
                dest.to_string(),
                src1.to_string(),
                src2.to_string()
            ),
            AsmLine::Srl(dest, src1, src2) => format!(
                "\tsrl {}, {}, {}",
                dest.to_string(),
                src1.to_string(),
                src2.to_string()
            ),
            AsmLine::Sra(dest, src1, src2) => format!(
                "\tsra {}, {}, {}",
                dest.to_string(),
                src1.to_string(),
                src2.to_string()
            ),
            AsmLine::Mul(dest, src1, src2) => format!(
                "\tmul {}, {}, {}",
                dest.to_string(),
                src1.to_string(),
                src2.to_string()
            ),
            AsmLine::Div(dest, src1, src2) => format!(
                "\tdiv {}, {}, {}",
                dest.to_string(),
                src1.to_string(),
                src2.to_string()
            ),
            AsmLine::Rem(dest, src1, src2) => format!(
                "\trem {}, {}, {}",
                dest.to_string(),
                src1.to_string(),
                src2.to_string()
            ),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RegLocation {
    Reg(RegName),
    Stack(usize),
}

impl Into<AsmValue> for RegLocation {
    fn into(self) -> AsmValue {
        match self {
            RegLocation::Reg(reg) => AsmValue::Reg(reg),
            RegLocation::Stack(offset) => AsmValue::Offset((offset * 4) as i32, RegName::Stack),
        }
    }
}

#[derive(Debug)]
struct RegisterAllocatorState {
    regs: BTreeMap<RegName, bool>,
    special_regs: BTreeMap<RegName, bool>,
    stack: Vec<bool>,
    mapping: HashMap<Value, RegLocation>,
}

impl RegisterAllocatorState {
    fn new() -> Self {
        let mut regs = BTreeMap::new();
        let mut special_regs = BTreeMap::new();
        for i in 1..6 {
            regs.insert(RegName::TempT(i), false);
        }
        for i in 2..12 {
            regs.insert(RegName::TempS(i), false);
        }
        for i in 0..8 {
            special_regs.insert(RegName::Param(i), false);
        }
        Self {
            regs,
            special_regs,
            stack: Vec::new(),
            mapping: HashMap::new(),
        }
    }

    fn free_location(&mut self, value: Value, location: RegLocation) {
        if self.mapping.get(&value) != Some(&location) {
            return;
        }
        self.mapping.remove(&value);
        match location {
            RegLocation::Reg(reg) => {
                if let Some(used) = self.regs.get_mut(&reg) {
                    *used = false;
                } else if let Some(used) = self.special_regs.get_mut(&reg) {
                    *used = false;
                } else {
                    panic!("Trying to free unknown register {:?}", reg);
                }
            }
            RegLocation::Stack(index) => {
                if let Some(used) = self.stack.get_mut(index) {
                    *used = false;
                }
            }
        }
    }

    fn alloc_stack_slot(&mut self) -> usize {
        for (index, used) in self.stack.iter_mut().enumerate() {
            if !*used {
                *used = true;
                return index;
            }
        }
        self.stack.push(true);
        self.stack.len() - 1
    }

    fn max_stack_size(&self) -> usize {
        self.stack.len()
    }
}

pub struct RegAddress {
    state: Rc<RefCell<RegisterAllocatorState>>,
    value: Value,
    pub location: RegLocation,
    owned: bool,
}

impl RegAddress {
    fn new(
        state: Rc<RefCell<RegisterAllocatorState>>,
        value: Value,
        location: RegLocation,
        owned: bool,
    ) -> Self {
        Self {
            state,
            value,
            location,
            owned,
        }
    }
}

impl Drop for RegAddress {
    fn drop(&mut self) {
        if !self.owned {
            return;
        }
        let mut state = self.state.borrow_mut();
        state.free_location(self.value, self.location.clone());
    }
}

impl Into<RegLocation> for RegAddress {
    fn into(self) -> RegLocation {
        self.location.clone()
    }
}

impl Into<RegName> for RegAddress {
    fn into(self) -> RegName {
        match self.location.clone() {
            RegLocation::Reg(reg) => reg,
            RegLocation::Stack(_) => panic!("Cannot convert stack location to register name"),
        }
    }
}

#[derive(Debug)]
pub struct RegisterAllocator {
    state: Rc<RefCell<RegisterAllocatorState>>,
}

impl RegisterAllocator {
    pub fn new() -> Self {
        Self {
            state: Rc::new(RefCell::new(RegisterAllocatorState::new())),
        }
    }

    pub fn find(&self, value: &Value) -> Option<RegAddress> {
        let state = self.state.borrow();
        let location = state.mapping.get(value)?.clone();
        Some(RegAddress::new(
            Rc::clone(&self.state),
            value.clone(),
            location,
            false,
        ))
    }

    pub fn register(&self, value: Value) -> RegAddress {
        let mut state = self.state.borrow_mut();
        // println!("Registering value {:?} in register allocator", value);
        if let Some(location) = state.mapping.get(&value).cloned() {
            // println!("Value {:?} is already mapped to location {:?}", value, location);
            return RegAddress::new(Rc::clone(&self.state), value, location, false);
        }

        let mut picked_reg: Option<RegName> = None;
        for (reg, used) in state.regs.iter_mut() {
            if !*used {
                *used = true;
                picked_reg = Some(reg.clone());
                break;
            }
        }

        if let Some(reg) = picked_reg {
            let location = RegLocation::Reg(reg);
            state.mapping.insert(value, location.clone());
            // println!("Mapped value {:?} to register {:?}", value, location);
            return RegAddress::new(Rc::clone(&self.state), value, location, true);
        }

        let stack_index = state.alloc_stack_slot();
        let location = RegLocation::Stack(stack_index);
        state.mapping.insert(value, location.clone());
        // println!("No free register for value {:?}, allocated stack slot {}", value, stack_index);
        RegAddress::new(Rc::clone(&self.state), value, location, true)
    }

    pub fn register_param(&self, value: Value, index: usize) -> RegAddress {
        let mut state = self.state.borrow_mut();
        let reg_name = RegName::Param(index);
        if state.special_regs.get(&reg_name).cloned().unwrap() {
            panic!("Parameter register {:?} is already used", reg_name);
        }
        let location = RegLocation::Reg(reg_name.clone());
        state.mapping.insert(value, location.clone());
        RegAddress::new(Rc::clone(&self.state), value, location, true)
    }

    pub fn max_stack_size(&self) -> usize {
        self.state.borrow().max_stack_size()
    }
}
