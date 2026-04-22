pub type Type = String;
pub type Label = String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KoopaLine {
    FuncDecl(String, String, Type), // @func(args): type
    FuncStart(String, String, Type),
    FuncEnd,
    Label(Label),
    ArgLabel(Label, Label, Type),      // %br(%a: type)
    GlobalAlloc(String, Type, String), // global = alloc type, init
    Alloc(String, Type),
    Store(String, String),
    Load(String, String),
    Binary(String, String, String, String),
    Br(String, Label, Label),
    Jump(Label),
    ArgJump(Label, String),       // jump %bb(%arg)
    Call(String, String, String), // %dest = call @func(%arg)
    VoidCall(String, String),     // call @func(%arg)
    Ret(String),
    VoidRet,
}

impl KoopaLine {
    pub fn to_string(&self) -> String {
        match self {
            KoopaLine::FuncDecl(name, args, ret_type) => {
                if matches!(ret_type.as_str(), "void") {
                    format!("decl @{}({})", name, args)
                } else {
                    format!("decl @{}({}): {}", name, args, ret_type)
                }
            }
            KoopaLine::FuncStart(name, args, ret_type) => {
                if matches!(ret_type.as_str(), "void") {
                    format!("fun @{}({}) {{", name, args)
                } else {
                    format!("fun @{}({}): {} {{", name, args, ret_type)
                }
            }
            KoopaLine::FuncEnd => "}\n".to_string(),
            KoopaLine::Label(label) => format!("{}:", label),
            KoopaLine::ArgLabel(label, arg_name, arg_type) => {
                format!("\n{}({}: {}):", label, arg_name, arg_type)
            }
            KoopaLine::GlobalAlloc(name, ty, init) => {
                format!("global {} = alloc {}, {}\n", name, ty, init)
            }
            KoopaLine::Alloc(ptr_name, ptr_type) => format!("\t{} = alloc {}", ptr_name, ptr_type),
            KoopaLine::Store(value, ptr) => format!("\tstore {}, {}", value, ptr),
            KoopaLine::Load(dest, ptr) => format!("\t{} = load {}", dest, ptr),
            KoopaLine::Binary(dest, op, lhs, rhs) => {
                format!("\t{} = {} {}, {}", dest, op, lhs, rhs)
            }
            KoopaLine::Br(cond, then_bb, else_bb) => {
                format!("\tbr {}, {}, {}", cond, then_bb, else_bb)
            }
            KoopaLine::Jump(target) => format!("\tjump {}", target),
            KoopaLine::ArgJump(target, arg) => format!("\tjump {}({})", target, arg),
            KoopaLine::Call(dest, func, arg) => format!("\t{} = call @{}({})", dest, func, arg),
            KoopaLine::VoidCall(func, arg) => format!("\tcall @{}({})", func, arg),
            KoopaLine::Ret(value) => format!("\tret {}", value),
            KoopaLine::VoidRet => format!("\tret"),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct KoopaLines {
    lines: Vec<KoopaLine>,
}

impl KoopaLines {
    pub fn new() -> Self {
        Self { lines: Vec::new() }
    }

    pub fn with_line(line: KoopaLine) -> Self {
        Self { lines: vec![line] }
    }

    fn is_closed(&self) -> bool {
        if let Some(last_line) = self.lines.last() {
            matches!(
                last_line,
                KoopaLine::Ret(_) | KoopaLine::Jump(_) | KoopaLine::Br(_, _, _)
            )
        } else {
            false
        }
    }

    pub fn close(&mut self) {
        if !self.is_closed() {
            self.add_line(KoopaLine::VoidRet);
        }
        self.add_line(KoopaLine::FuncEnd);
    }

    pub fn make_block(origin: KoopaLines, this_bb: String, next_bb: Option<String>) -> Self {
        let mut block_lines = Self::with_line(KoopaLine::Label(this_bb));
        block_lines.add_lines(origin);
        if !block_lines.is_closed() {
            if let Some(next_bb) = next_bb {
                block_lines.add_line(KoopaLine::Jump(next_bb));
            } else {
                panic!("Block is not closed and no next block provided");
            }
        }
        block_lines
    }

    pub fn add_line(&mut self, line: KoopaLine) {
        self.lines.push(line);
    }

    pub fn add_lines(&mut self, mut other: KoopaLines) {
        self.lines.append(&mut other.lines);
    }

    pub fn to_string(&self) -> String {
        self.lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }
}
