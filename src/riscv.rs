use std::collections::HashMap;

use koopa::ir::Value;

#[derive(Eq, Hash, PartialEq, Clone)]
pub enum AsmValue {
    Ret,
    Param(usize),
    Temp(String),
    Const(i32),
    Zero,
}

impl AsmValue {
    pub fn to_string(&self) -> String {
        match self {
            AsmValue::Ret => "a0".to_string(),
            AsmValue::Param(i) => format!("a{}", i + 1),
            AsmValue::Temp(name) => name.clone(),
            AsmValue::Const(imm) => imm.to_string(),
            AsmValue::Zero => "zero".to_string(),
        }
    }
}

pub enum Address {
    Reg(AsmValue),
    Offset(i32, AsmValue),
    Stack(i32),
}

impl Address {
    pub fn to_string(&self) -> String {
        match self {
            Address::Reg(reg) => reg.to_string(),
            Address::Offset(offset, reg) => format!("{}({})", offset, reg.to_string()),
            Address::Stack(offset) => format!("{}(sp)", offset),
        }
    }
}

pub enum AsmLine {
// Control flow
    Label(i32),
    Jump(i32),
    Ret,
    Call(String),
// Data movement
    Mv(AsmValue, AsmValue),
    Load(AsmValue, Address),
    Store(Address, AsmValue),
    Li(AsmValue, i32),
    La(AsmValue, String),
// Arithmetic
    Add(AsmValue, AsmValue, AsmValue),
    Addi(AsmValue, AsmValue, i32),
    Sub(AsmValue, AsmValue, AsmValue),
    Slt(AsmValue, AsmValue, AsmValue),
    Sgt(AsmValue, AsmValue, AsmValue),
    Seqz(AsmValue, AsmValue),
    Snez(AsmValue, AsmValue),
    Xor(AsmValue, AsmValue, AsmValue),
    Xori(AsmValue, AsmValue, i32),
    Or(AsmValue, AsmValue, AsmValue),
    Ori(AsmValue, AsmValue, i32),
    And(AsmValue, AsmValue, AsmValue),
    Andi(AsmValue, AsmValue, i32),
    Sll(AsmValue, AsmValue, AsmValue),
    Srl(AsmValue, AsmValue, AsmValue),
    Sra(AsmValue, AsmValue, AsmValue),
    Mul(AsmValue, AsmValue, AsmValue),
    Div(AsmValue, AsmValue, AsmValue),
    Rem(AsmValue, AsmValue, AsmValue),
}

impl AsmLine {
    pub fn to_string(&self) -> String {
        match self {
        // Control flow
            AsmLine::Label(id)
                => format!("L{}:", id),
            AsmLine::Jump(id)
                => format!("\tj L{}", id),
            AsmLine::Ret
                => "\tret".to_string(),
            AsmLine::Call(func)
                => format!("\tcall {}", func),
        // Data movement
            AsmLine::Mv(dest, src)
                => format!("\tmv {}, {}", dest.to_string(), src.to_string()),
            AsmLine::Load(dest, addr)
                => format!("\tlw {}, {}", dest.to_string(), addr.to_string()),
            AsmLine::Store(addr, src)
                => format!("\tsw {}, {}", src.to_string(), addr.to_string()),
            AsmLine::La(dest, label)
                => format!("\tla {}, {}", dest.to_string(), label),
            AsmLine::Li(dest, imm)
                => format!("\tli {}, {}", dest.to_string(), imm),
        // Arithmetic
            AsmLine::Add(dest, src1, src2)
                => format!("\tadd {}, {}, {}", dest.to_string(), src1.to_string(), src2.to_string()),
            AsmLine::Addi(dest, src, imm)
                => format!("\taddi {}, {}, {}", dest.to_string(), src.to_string(), imm),
            AsmLine::Sub(dest, src1, src2)
                => format!("\tsub {}, {}, {}", dest.to_string(), src1.to_string(), src2.to_string()),
            AsmLine::Slt(dest, src1, src2)
                => format!("\tslt {}, {}, {}", dest.to_string(), src1.to_string(), src2.to_string()),
            AsmLine::Sgt(dest, src1, src2)
                => format!("\tsgt {}, {}, {}", dest.to_string(), src1.to_string(), src2.to_string()),
            AsmLine::Seqz(dest, src)
                => format!("\tseqz {}, {}", dest.to_string(), src.to_string()),
            AsmLine::Snez(dest, src)
                => format!("\tsnez {}, {}", dest.to_string(), src.to_string()),
            AsmLine::Xor(dest, src1, src2)
                => format!("\txor {}, {}, {}", dest.to_string(), src1.to_string(), src2.to_string()),
            AsmLine::Xori(dest, src, imm)
                => format!("\txori {}, {}, {}", dest.to_string(), src.to_string(), imm),
            AsmLine::Or(dest, src1, src2)
                => format!("\tor {}, {}, {}", dest.to_string(), src1.to_string(), src2.to_string()),
            AsmLine::Ori(dest, src, imm)
                => format!("\tori {}, {}, {}", dest.to_string(), src.to_string(), imm),
            AsmLine::And(dest, src1, src2)
                => format!("\tand {}, {}, {}", dest.to_string(), src1.to_string(), src2.to_string()),
            AsmLine::Andi(dest, src, imm)
                => format!("\tandi {}, {}, {}", dest.to_string(), src.to_string(), imm),
            AsmLine::Sll(dest, src1, src2)
                => format!("\tsll {}, {}, {}", dest.to_string(), src1.to_string(), src2.to_string()),
            AsmLine::Srl(dest, src1, src2)
                => format!("\tsrl {}, {}, {}", dest.to_string(), src1.to_string(), src2.to_string()),
            AsmLine::Sra(dest, src1, src2)
                => format!("\tsra {}, {}, {}", dest.to_string(), src1.to_string(), src2.to_string()),
            AsmLine::Mul(dest, src1, src2)
                => format!("\tmul {}, {}, {}", dest.to_string(), src1.to_string(), src2.to_string()),
            AsmLine::Div(dest, src1, src2)
                => format!("\tdiv {}, {}, {}", dest.to_string(), src1.to_string(), src2.to_string()),
            AsmLine::Rem(dest, src1, src2)
                => format!("\trem {}, {}, {}", dest.to_string(), src1.to_string(), src2.to_string()),
        }
    }
}

pub struct RegisterAllocator {
    regs: HashMap<AsmValue, bool>,
    stack: Vec<Option<bool>>,
    mapping: HashMap<Value, Result<AsmValue, usize>>,
}

impl RegisterAllocator {
    pub fn new() -> Self {
        let mut regs = HashMap::new();
        for i in 0..6 {
            regs.insert(AsmValue::Temp(format!("t{}", i)), false);
        }
        for i in 2..12 {
            regs.insert(AsmValue::Temp(format!("s{}", i)), false);
        }
        Self { regs, stack: Vec::new(), mapping: HashMap::new()}
    }

    pub fn find(&self, value: &Value) -> Option<Result<AsmValue, usize>> {
        self.mapping.get(value).cloned()
    }

    pub fn register(&mut self, value: Value) -> Result<AsmValue, usize> {
        if let Some(reg) = self.mapping.get(&value) {
            return reg.clone();
        }
        for (reg, used) in self.regs.iter_mut() {
            if !*used {
                *used = true;
                self.mapping.insert(value, Ok(reg.clone()));
                return Ok(reg.clone());
            }
        }
        for (i, used) in self.stack.iter_mut().enumerate() {
            if used.is_none() {
                *used = Some(true);
                self.mapping.insert(value, Err(i));
                return Err(i);
            }
        }
        self.stack.push(Some(true));
        let index = self.stack.len() - 1;
        self.mapping.insert(value, Err(index));
        Err(index)
    }

    pub fn free(&mut self, value: Value) {
        if let Some(reg) = self.mapping.get(&value) {
            match reg {
                Ok(r) => {
                    self.regs.insert(r.clone(), false);
                }
                Err(i) => {
                    if *i < self.stack.len() {
                        self.stack[*i] = None;
                    }
                }
            }
        }
    }

    pub fn free_all(&mut self) {
        for used in self.regs.values_mut() {
            *used = false;
        }
        for used in self.stack.iter_mut() {
            *used = None;
        }
        self.mapping.clear();
    }

    pub fn max_stack_size(&self) -> usize {
        self.stack.len()
    }
}