use koopa::ir::dfg::DataFlowGraph;
use koopa::ir::values::{Binary, BinaryOp};
use koopa::ir::{Value, ValueKind};
use koopa::ir::{FunctionData, Program};
use crate::riscv::{AsmLine, AsmValue, RegisterAllocator};

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

impl GenerateAsm for &FunctionData {
    fn to_asm(&self, bg: &Background) -> Asm {
        let mut result = Asm::new();
        let mut context = Asm::new();
        let name = name_to_symbol(self.name());
        result.add_text("\t.text".to_string());
        result.add_text(format!("\t.globl {}", name));
        result.add_text(format!("{}:", name));
        let mut ra =  RegisterAllocator::new();
        for (&_bb, node) in self.layout().bbs() {
            let insts = node.insts().keys();
            for &inst in insts {
                context.add_asm(gen_inst_asm(inst, &bg.with_dfg(self.dfg()), &mut ra).asm);
            }
            ra.free_all();
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
        result
    }
}

#[allow(unused)]
pub struct InstAsm {
    asm: Asm,
    ret: Option<AsmValue>,
}

fn gen_inst_asm(value: Value, bg: &Background, ra: &mut RegisterAllocator) -> InstAsm {
    let inst = bg.dfg.unwrap().value(value);
    match inst.kind() {
        ValueKind::Return(r) => {
            let mut asm = Asm::new();
            if let Some(value) = r.value() {
                let ret_val = single_value_to_asm(value, bg, ra);
                asm.add_line(mov_to_asm(AsmValue::Ret, ret_val, bg, ra));
            }
            InstAsm { asm, ret: None }
        }
        ValueKind::Integer(_) => {
            panic!("Integer value should not be directly used as an instruction");
        }
        ValueKind::Binary(binary) => {
            let asm = binary_to_asm(value, binary, bg, ra);
            InstAsm { asm, ret: Some(ra.register(value).unwrap()) }
        }
        _ => unimplemented!()
    }
}

fn single_value_to_asm(value: Value, bg: &Background, ra: &mut RegisterAllocator) -> AsmValue {
    let inst = bg.dfg.unwrap().value(value);
    match inst.kind() {
        ValueKind::Integer(i) => {
            AsmValue::Const(i.value())
        }
        _ => {
            let result = ra.find(&value).unwrap();
            match result {
                Ok(reg) => reg,
                Err(_) => unimplemented!("Value {:?} is not allocated in a register", value),
            }
        }
    }
}

fn mov_to_asm(reg: AsmValue, value: AsmValue, bg: &Background, ra: &mut RegisterAllocator) -> AsmLine {
    match value {
        AsmValue::Const(c) => AsmLine::Li(reg, c),
        AsmValue::Temp(_) => AsmLine::Mv(reg, value),
        _ => unimplemented!(),
    }
}

fn binary_to_asm(value: Value, binary: &Binary, bg: &Background, ra: &mut RegisterAllocator) -> Asm {
    let mut asm = Asm::new();
    let lhs = single_value_to_asm(binary.lhs(), bg, ra);
    let rhs = single_value_to_asm(binary.rhs(), bg, ra);
    let dest = ra.register(value).unwrap();
    match binary.op() {
        BinaryOp::Add => {
            asm.add_line(AsmLine::Add(dest, lhs, rhs));
        }
        BinaryOp::Sub => {
            asm.add_line(AsmLine::Sub(dest, lhs, rhs));
        }
        BinaryOp::Mul => {
            asm.add_line(AsmLine::Mul(dest, lhs, rhs));
        }
        BinaryOp::Div => {
            asm.add_line(AsmLine::Div(dest, lhs, rhs));
        }
        BinaryOp::Mod => {
            asm.add_line(AsmLine::Rem(dest, lhs, rhs));
        }
        BinaryOp::Eq => {
            asm.add_line(mov_to_asm(dest.clone(), lhs, bg, ra));
            asm.add_line(AsmLine::Xor(dest.clone(), dest.clone(), rhs));
            asm.add_line(AsmLine::Seqz(dest.clone(), dest));
        }
        _ => unimplemented!("{}", format!("Unsupported binary operation: {:?}", binary.op())),
    }
    asm
}