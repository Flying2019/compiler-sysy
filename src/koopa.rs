pub type Label = String;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KoopaType {
    I32,
    Void,
    Struct(String),
    Ptr(Box<KoopaType>),
    Array(Box<KoopaType>, usize),
}

impl KoopaType {
    pub fn ptr(base: KoopaType) -> Self {
        Self::Ptr(Box::new(base))
    }

    pub fn array(base: KoopaType, len: usize) -> Self {
        Self::Array(Box::new(base), len)
    }

    pub fn display_wrapped(&self) -> String {
        match self {
            KoopaType::I32 => "i32".to_string(),
            KoopaType::Void => "void".to_string(),
            KoopaType::Struct(name) => format!("struct @{}", name),
            KoopaType::Ptr(inner) => format!("*{}", inner.display_wrapped()),
            KoopaType::Array(inner, len) => format!("[{}, {}]", inner.display_wrapped(), len),
        }
    }

}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KoopaParam {
    pub name: String,
    pub ty: KoopaType,
}

impl KoopaParam {
    pub fn new(name: String, ty: KoopaType) -> Self {
        Self { name, ty }
    }

    fn display_wrapped(&self) -> String {
        format!("{}: {}", self.name, self.ty.display_wrapped())
    }

    fn wrapped_type(&self) -> String {
        self.ty.display_wrapped()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KoopaField {
    pub name: String,
    pub ty: KoopaType,
}

impl KoopaField {
    pub fn new(name: String, ty: KoopaType) -> Self {
        Self { name, ty }
    }

    fn display_wrapped(&self) -> String {
        format!("{} {}", self.ty.display_wrapped(), self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KoopaLine {
    StructDecl(String, Vec<KoopaField>),
    FuncDecl(String, Vec<KoopaParam>, KoopaType),
    FuncStart(String, Vec<KoopaParam>, KoopaType),
    FuncEnd,
    Label(Label),
    ArgLabel(Label, Label, KoopaType),         // %br(%a: type)
    GlobalAlloc(String, KoopaType, String),    // global = alloc type, init
    Alloc(String, KoopaType),
    Store(String, String),
    Load(String, String),
    GetPtr(String, String, String),
    GetElemPtr(String, String, String),
    Binary(String, String, String, String),
    Br(String, Label, Label),
    Jump(Label),
    ArgJump(Label, String),                    // jump %bb(%arg)
    Call(String, String, String),              // %dest = call @func(%arg)
    VoidCall(String, String),                  // call @func(%arg)
    Ret(String),
    VoidRet,
}

impl KoopaLine {
    pub fn to_wrapped_string(&self) -> String {
        match self {
            KoopaLine::StructDecl(name, fields) => {
                let fields = fields
                    .iter()
                    .map(KoopaField::display_wrapped)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("type @{} = {{ {} }}", name, fields)
            }
            KoopaLine::FuncDecl(name, args, ret_type) => {
                let args = args
                    .iter()
                    .map(KoopaParam::wrapped_type)
                    .collect::<Vec<_>>()
                    .join(", ");
                if matches!(ret_type, KoopaType::Void) {
                    format!("decl @{}({})", name, args)
                } else {
                    format!("decl @{}({}): {}", name, args, ret_type.display_wrapped())
                }
            }
            KoopaLine::FuncStart(name, args, ret_type) => {
                let args = args
                    .iter()
                    .map(KoopaParam::display_wrapped)
                    .collect::<Vec<_>>()
                    .join(", ");
                if matches!(ret_type, KoopaType::Void) {
                    format!("fun @{}({}) {{", name, args)
                } else {
                    format!("fun @{}({}): {} {{", name, args, ret_type.display_wrapped())
                }
            }
            KoopaLine::FuncEnd => "}\n".to_string(),
            KoopaLine::Label(label) => format!("{}:", label),
            KoopaLine::ArgLabel(label, arg_name, arg_type) => {
                format!("\n{}({}: {}):", label, arg_name, arg_type.display_wrapped())
            }
            KoopaLine::GlobalAlloc(name, ty, init) => {
                format!("global {} = alloc {}, {}\n", name, ty.display_wrapped(), init)
            }
            KoopaLine::Alloc(ptr_name, ptr_type) => {
                format!("\t{} = alloc {}", ptr_name, ptr_type.display_wrapped())
            }
            KoopaLine::Store(value, ptr) => format!("\tstore {}, {}", value, ptr),
            KoopaLine::Load(dest, ptr) => format!("\t{} = load {}", dest, ptr),
            KoopaLine::GetPtr(dest, ptr, idx) => format!("\t{} = getptr {}, {}", dest, ptr, idx),
            KoopaLine::GetElemPtr(dest, ptr, idx) => {
                format!("\t{} = getelemptr {}, {}", dest, ptr, idx)
            }
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
                KoopaLine::Ret(_) | KoopaLine::VoidRet | KoopaLine::Jump(_) | KoopaLine::Br(_, _, _)
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

    pub fn lines(&self) -> &[KoopaLine] {
        &self.lines
    }

    pub fn to_wrapped_string(&self) -> String {
        self.lines
            .iter()
            .map(KoopaLine::to_wrapped_string)
            .collect::<Vec<_>>()
            .join("\n")
    }
}
