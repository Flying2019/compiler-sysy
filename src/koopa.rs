#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KoopaLine {
    FuncStart(String, String),
    FuncEnd,
    Label(String),
    ArgLabel(String, String, String), // %br(%a: type)
    Alloc(String),
    Store(String, String),
    Load(String, String),
    Binary(String, String, String, String),
    Br(String, String, String),
    Jump(String),
    ArgJump(String, String), // jump %bb(%arg)
    Ret(String),
}

impl KoopaLine {
    pub fn to_string(&self) -> String {
        match self {
            KoopaLine::FuncStart(name, ret_type) => format!("fun @{}(): {} {{", name, ret_type),
            KoopaLine::FuncEnd => "}".to_string(),
            KoopaLine::Label(label) => format!("\n{}:", label),
            KoopaLine::ArgLabel(label, arg_name, arg_type) => format!("\n{}({}: {}):", label, arg_name, arg_type),
            KoopaLine::Alloc(ptr) => format!("\t{} = alloc i32", ptr),
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
            KoopaLine::Ret(value) => format!("\tret {}", value),
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
            matches!(last_line, KoopaLine::Ret(_) | KoopaLine::Jump(_) | KoopaLine::Br(_, _, _))
        } else {
            false
        }
    }

    pub fn remove_last_label(&mut self) {
        if let Some(KoopaLine::Label(_)) = self.lines.last() {
            self.lines.pop();
        }
    }

    pub fn make_block(origin: KoopaLines, this_bb: String, next_bb: Option<String>) -> Self {
        let mut block_lines = Self::with_line(KoopaLine::Label(this_bb));
        block_lines.add_lines(origin);
        if !block_lines.is_closed() {
            if let Some(next_bb) = next_bb {
                block_lines.add_line(KoopaLine::Jump(next_bb));
            }
            else {
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
