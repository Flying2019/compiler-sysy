use std::collections::HashMap;

use crate::koopa::{KoopaLine, KoopaLines, KoopaType};
use crate::lalr::VarDecl;
use crate::{
    ast::{AstNode, ReturnValue},
    lalr::{
        eval_param_dim, func_param_btype, BType, BinaryOp, CompUnit, Exp, FuncParam, GlobalDef,
        StructDef, Type, UnaryOp,
    },
};

const WORD_SIZE: usize = 4;

fn align_up(value: usize, align: usize) -> usize {
    if align == 0 {
        return value;
    }
    (value + align - 1) / align * align
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueType {
    Int,
    Struct(String),
    Pointer(Box<ValueType>),
    Array(usize, Box<ValueType>),
}

impl ValueType {
    pub fn from_btype(btype: &BType) -> Self {
        match btype {
            BType::I32 => ValueType::Int,
            BType::Struct(name) => ValueType::Struct(name.clone()),
            // Compatibility async lowering treats Promise<T> as a synchronous T value.
            // The real Promise frame lowering will replace this once the async runtime lands.
            BType::Promise(inner) => ValueType::from_btype(inner),
            BType::Ptr(inner) => ValueType::Pointer(Box::new(ValueType::from_btype(inner))),
            BType::Array(len, inner) => {
                ValueType::Array(*len, Box::new(ValueType::from_btype(inner)))
            }
            BType::Void => panic!("Void is not a storable value type"),
        }
    }

    pub fn align(&self, bg: &Background) -> usize {
        match self {
            ValueType::Int | ValueType::Pointer(_) => WORD_SIZE,
            ValueType::Array(_, inner) => inner.align(bg),
            ValueType::Struct(name) => bg.get_struct_layout(name).align,
        }
    }

    pub fn size(&self, bg: &Background) -> usize {
        match self {
            ValueType::Int | ValueType::Pointer(_) => WORD_SIZE,
            ValueType::Array(len, inner) => inner.size(bg) * len,
            ValueType::Struct(name) => bg.get_struct_layout(name).size,
        }
    }

    pub fn slot_count(&self, bg: &Background) -> usize {
        align_up(self.size(bg), WORD_SIZE) / WORD_SIZE
    }

    pub fn from_dims(dims: &[usize]) -> Self {
        let mut ty = ValueType::Int;
        for len in dims.iter().rev() {
            ty = ValueType::Array(*len, Box::new(ty));
        }
        ty
    }

    pub fn to_koopa_type(&self, _bg: &Background) -> KoopaType {
        match self {
            ValueType::Int => KoopaType::I32,
            ValueType::Struct(name) => KoopaType::Struct(name.clone()),
            ValueType::Pointer(inner) => KoopaType::ptr(inner.to_koopa_type(_bg)),
            ValueType::Array(len, inner) => KoopaType::array(inner.to_koopa_type(_bg), *len),
        }
    }
}

#[derive(Debug, Clone)]
pub struct StructFieldLayout {
    pub name: String,
    pub ty: ValueType,
    pub offset: usize,
}

#[derive(Debug, Clone)]
pub struct StructLayout {
    pub name: String,
    pub fields: Vec<StructFieldLayout>,
    pub size: usize,
    pub align: usize,
}

#[derive(Debug)]
pub struct RenameManager {
    timestamp: usize,
    rename_count: HashMap<String, usize>,
    rename_record: HashMap<String, Vec<(usize, usize)>>, // (timestamp, count)
}

fn variable_rename(name: String, count: usize) -> String {
    if count == 1 {
        format!("@{}", name)
    } else {
        format!("@{}_{}", name, count)
    }
}

impl RenameManager {
    pub fn new() -> Self {
        Self {
            timestamp: 0,
            rename_count: HashMap::new(),
            rename_record: HashMap::new(),
        }
    }

    pub fn new_variable(&mut self, name: String) -> Option<usize> {
        let count = self.rename_count.entry(name.clone()).or_insert(0);
        *count += 1;
        let times = self.rename_record.entry(name).or_insert(Vec::new());
        if let Some(last) = times.last() {
            if last.0 == self.timestamp {
                return None;
            }
        }
        times.push((self.timestamp, *count));
        Some(*count)
    }

    pub fn get_current_name(&self, name: &String) -> Option<usize> {
        let &(ts, count) = self.rename_record.get(name)?.last()?;
        if ts <= self.timestamp {
            Some(count)
        } else {
            // The variable is not visible
            None
        }
    }

    pub fn rollback(&mut self, timestamp: usize) {
        for (_, times) in self.rename_record.iter_mut() {
            while let Some(&(ts, _)) = times.last() {
                if ts > timestamp {
                    times.pop();
                } else {
                    break;
                }
            }
        }
    }

    pub fn increment(&mut self) {
        self.timestamp += 1;
    }

    pub fn record(&self) -> usize {
        self.timestamp
    }
}

type NextBb = Option<String>;

#[derive(Debug)]
struct GlobalSymbols {
    global_variable: HashMap<String, ValueType>,
    global_function: HashMap<String, (Type, Vec<BType>)>, // function name -> (return type, parameter types)
    static_function: HashMap<String, (Type, Vec<BType>)>, // function name -> (return type, parameter types)
    async_function: HashMap<String, bool>,
}

impl GlobalSymbols {
    pub fn new() -> Self {
        Self {
            global_variable: HashMap::new(),
            global_function: HashMap::new(),
            static_function: HashMap::new(),
            async_function: HashMap::new(),
        }
    }
}

#[derive(Debug)]
pub struct Background {
    global_symbols: GlobalSymbols,
    struct_defs: HashMap<String, StructDef>,
    struct_layouts: HashMap<String, StructLayout>,
    temp_counter: usize,
    branch_counter: usize,
    constant_map: HashMap<String, i32>,
    variable_types: HashMap<String, ValueType>,
    rename_manager: RenameManager,
    next_bb: NextBb,
    loop_next: NextBb,
    loop_entry: NextBb,
}

impl Background {
    pub fn new() -> Self {
        Self {
            global_symbols: GlobalSymbols::new(),
            struct_defs: HashMap::new(),
            struct_layouts: HashMap::new(),
            temp_counter: 0,
            branch_counter: 0,
            constant_map: HashMap::new(),
            variable_types: HashMap::new(),
            rename_manager: RenameManager::new(),
            next_bb: None,
            loop_next: None,
            loop_entry: None,
        }
    }

    pub fn clear(&mut self) {
        self.next_bb = None;
        self.loop_next = None;
        self.loop_entry = None;
    }

    pub fn register_struct_def(&mut self, def: StructDef) {
        if self.struct_defs.contains_key(&def.name) {
            panic!("Duplicate struct definition: {}", def.name);
        }
        self.struct_defs.insert(def.name.clone(), def);
    }

    pub fn get_struct_layout(&self, name: &str) -> &StructLayout {
        self.struct_layouts
            .get(name)
            .unwrap_or_else(|| panic!("Struct layout not resolved yet: {}", name))
    }

    pub fn get_struct_field(&self, struct_name: &str, field_name: &str) -> &StructFieldLayout {
        self.get_struct_layout(struct_name)
            .fields
            .iter()
            .find(|field| field.name == field_name)
            .unwrap_or_else(|| panic!("Struct {} has no field named {}", struct_name, field_name))
    }

    pub fn resolve_struct_layouts(&mut self) {
        let names = self.struct_defs.keys().cloned().collect::<Vec<_>>();
        let mut visiting = Vec::new();
        for name in names {
            self.ensure_struct_layout(&name, &mut visiting);
        }
    }

    fn ensure_struct_layout(&mut self, name: &str, visiting: &mut Vec<String>) -> StructLayout {
        if let Some(layout) = self.struct_layouts.get(name) {
            return layout.clone();
        }
        if visiting.iter().any(|item| item == name) {
            panic!("Cyclic struct definition detected: {}", name);
        }
        visiting.push(name.to_string());
        let def = self
            .struct_defs
            .get(name)
            .unwrap_or_else(|| panic!("Unknown struct type: {}", name))
            .clone();

        let mut fields = Vec::new();
        let mut offset = 0usize;
        let mut align = WORD_SIZE;
        for field in def.fields {
            for decl in field.decls {
                let field_ty = resolve_decl_value_type(&field.ty, &decl.var, self);
                let field_name = flatten_var_decl(&decl.var, self)
                    .map(|(name, _)| name)
                    .unwrap_or_else(|| panic!("Array size must be a constant expression"));
                let field_align = field_ty.align(self);
                let field_size = field_ty.size(self);
                align = align.max(field_align);
                offset = align_up(offset, field_align);
                fields.push(StructFieldLayout {
                    name: field_name,
                    ty: field_ty,
                    offset,
                });
                offset += field_size;
            }
        }
        let size = align_up(offset, align);
        visiting.pop();
        let layout = StructLayout {
            name: name.to_string(),
            fields,
            size,
            align,
        };
        self.struct_layouts.insert(name.to_string(), layout.clone());
        layout
    }

    pub fn get_global_variable(&self, name: String) -> Option<&ValueType> {
        self.global_symbols.global_variable.get(&name)
    }

    pub fn get_function(&self, name: String) -> Option<&(Type, Vec<BType>)> {
        if let Some(func) = self.global_symbols.global_function.get(&name) {
            Some(func)
        } else {
            self.global_symbols.static_function.get(&name)
        }
    }

    pub fn is_async_function(&self, name: &str) -> bool {
        self.global_symbols
            .async_function
            .get(name)
            .copied()
            .unwrap_or(false)
    }

    pub fn get_static_functions_decl(&self) -> Vec<(String, Type, Vec<BType>)> {
        self.global_symbols
            .static_function
            .iter()
            .map(|(name, (func_type, params))| (name.clone(), func_type.clone(), params.clone()))
            .collect()
    }

    pub fn next_temp(&mut self) -> String {
        let temp_name = format!("%{}", self.temp_counter);
        self.rename_manager
            .new_variable(temp_name.clone())
            .expect("Temp variable name conflict");
        self.temp_counter += 1;
        temp_name
    }

    pub fn get_variable(&self, name: String) -> String {
        let count = self
            .rename_manager
            .get_current_name(&name)
            .expect(&format!("Variable {} not found in the current scope", name));
        variable_rename(name, count)
    }

    pub fn get_variable_type(&self, name: String) -> ValueType {
        let renamed = self.get_variable(name);
        self.variable_types
            .get(&renamed)
            .cloned()
            .expect("Variable type not found")
    }

    pub fn set_variable_type(&mut self, renamed: String, ty: ValueType) {
        self.variable_types.insert(renamed, ty);
    }

    pub fn new_variable(&mut self, name: String) -> String {
        let count = self
            .rename_manager
            .new_variable(name.clone())
            .expect("Variable name conflict in the same scope");
        let new_name = variable_rename(name.clone(), count);
        new_name
    }

    pub fn try_get_constant(&self, name: String) -> Option<i32> {
        let count = self.rename_manager.get_current_name(&name)?;
        let renamed = variable_rename(name, count);
        self.constant_map.get(&renamed).cloned()
    }

    pub fn set_constant(&mut self, name: String, value: i32) {
        let name = self.new_variable(name);
        self.constant_map.insert(name, value);
    }

    pub fn increment_time(&mut self) {
        self.rename_manager.increment();
    }

    pub fn record(&self) -> usize {
        self.rename_manager.record()
    }

    pub fn rollback(&mut self, timestamp: usize) {
        self.rename_manager.rollback(timestamp);
    }

    pub fn new_bb(&mut self) -> String {
        let branch_name = format!("%bb{}", self.branch_counter);
        self.branch_counter += 1;
        branch_name
    }

    pub fn next_bb(&self) -> NextBb {
        self.next_bb.clone()
    }

    pub fn set_next_bb(&mut self, bb: NextBb) {
        self.next_bb = bb;
    }

    pub fn new_next_bb_if_none(&mut self) {
        if self.next_bb.is_none() {
            let new_bb = self.new_bb();
            self.next_bb = Some(new_bb);
        }
    }

    pub fn get_loop_next(&self) -> NextBb {
        self.loop_next.clone()
    }

    pub fn get_loop_entry(&self) -> NextBb {
        self.loop_entry.clone()
    }

    pub fn set_loop(&mut self, next: NextBb, entry: NextBb) {
        self.loop_next = next;
        self.loop_entry = entry;
    }
}

pub fn gen_unary_koopa_ir(op: &UnaryOp, src: String, background: &mut Background) -> ReturnValue {
    let dst = background.next_temp();
    let line = match op {
        UnaryOp::Pos => KoopaLine::Binary(dst.clone(), "add".to_string(), "0".to_string(), src),
        UnaryOp::Neg => KoopaLine::Binary(dst.clone(), "sub".to_string(), "0".to_string(), src),
        UnaryOp::Not => KoopaLine::Binary(dst.clone(), "eq".to_string(), src, "0".to_string()),
        UnaryOp::Addr | UnaryOp::Deref => {
            panic!("Unary operator {:?} should be handled in Exp::UnaryExp.to_koopa(), not in gen_unary_koopa_ir()", op)
        }
    };
    ReturnValue {
        value: Some(dst),
        content: KoopaLines::with_line(line),
    }
}

pub fn gen_binary_koopa_ir(
    op: &BinaryOp,
    lhs: &Box<Exp>,
    rhs: &Box<Exp>,
    bg: &mut Background,
) -> ReturnValue {
    let dst = bg.next_temp();
    let lhs_ret = lhs.to_koopa(bg);
    let rhs_ret = rhs.to_koopa(bg);
    let lhs_src = lhs_ret.value.clone().unwrap();
    let rhs_src = rhs_ret.value.clone().unwrap();
    match op {
        BinaryOp::And | BinaryOp::Or => {
            // These logical operations require short-circuit evaluation, so we need to generate more complex IR.
            let mut ir = lhs_ret.content;

            let br_label = bg.new_bb();
            let br_then = format!("{}_then", br_label);
            let br_else = format!("{}_else", br_label);
            let br_end = format!("{}_end", br_label);
            // Using ArgLabel to implement SSA form for short-circuit evaluation
            if matches!(op, BinaryOp::And) {
                ir.add_line(KoopaLine::Br(
                    lhs_src.clone(),
                    br_then.clone(),
                    br_else.clone(),
                ));
            } else {
                ir.add_line(KoopaLine::Br(
                    lhs_src.clone(),
                    br_else.clone(),
                    br_then.clone(),
                ));
            }
            // Then block
            ir.add_line(KoopaLine::Label(br_then));
            ir.add_lines(rhs_ret.content);
            let tmp = bg.next_temp();
            ir.add_lines(KoopaLines::with_line(KoopaLine::Binary(
                tmp.clone(),
                "ne".to_string(),
                rhs_src.clone(),
                "0".to_string(),
            )));
            ir.add_line(KoopaLine::ArgJump(br_end.clone(), tmp));
            // Else block
            ir.add_line(KoopaLine::Label(br_else));
            if matches!(op, BinaryOp::And) {
                ir.add_line(KoopaLine::ArgJump(br_end.clone(), 0.to_string()));
            } else {
                ir.add_line(KoopaLine::ArgJump(br_end.clone(), 1.to_string()));
            }
            // End block
            ir.add_line(KoopaLine::ArgLabel(br_end, dst.clone(), KoopaType::I32));
            ReturnValue {
                value: Some(dst),
                content: ir,
            }
        }
        _ => {
            let op_name = match op {
                BinaryOp::Add => "add",
                BinaryOp::Sub => "sub",
                BinaryOp::Mul => "mul",
                BinaryOp::Div => "div",
                BinaryOp::Mod => "mod",
                BinaryOp::Lt => "lt",
                BinaryOp::Gt => "gt",
                BinaryOp::Le => "le",
                BinaryOp::Ge => "ge",
                BinaryOp::Eq => "eq",
                BinaryOp::Ne => "ne",
                _ => unimplemented!("Unsupported binary operation: {:?}", op),
            };
            let mut ir = lhs_ret.content;
            ir.add_lines(rhs_ret.content);
            ir.add_line(KoopaLine::Binary(
                dst.clone(),
                op_name.to_string(),
                lhs_src,
                rhs_src,
            ));
            ReturnValue {
                value: Some(dst),
                content: ir,
            }
        }
    }
}

pub fn eval_unary_const(op: &UnaryOp, value: i32) -> i32 {
    match op {
        UnaryOp::Pos => value,
        UnaryOp::Neg => -value,
        UnaryOp::Not => {
            if value == 0 {
                1
            } else {
                0
            }
        }
        UnaryOp::Addr | UnaryOp::Deref => {
            unimplemented!("Unary operator {:?} is not a constant expression", op)
        }
    }
}

pub fn eval_binary_const(op: &BinaryOp, lhs: i32, rhs: i32) -> i32 {
    match op {
        BinaryOp::Add => lhs + rhs,
        BinaryOp::Sub => lhs - rhs,
        BinaryOp::Mul => lhs * rhs,
        BinaryOp::Div => lhs / rhs,
        BinaryOp::Mod => lhs % rhs,
        BinaryOp::Lt => {
            if lhs < rhs {
                1
            } else {
                0
            }
        }
        BinaryOp::Gt => {
            if lhs > rhs {
                1
            } else {
                0
            }
        }
        BinaryOp::Le => {
            if lhs <= rhs {
                1
            } else {
                0
            }
        }
        BinaryOp::Ge => {
            if lhs >= rhs {
                1
            } else {
                0
            }
        }
        BinaryOp::Eq => {
            if lhs == rhs {
                1
            } else {
                0
            }
        }
        BinaryOp::Ne => {
            if lhs != rhs {
                1
            } else {
                0
            }
        }
        BinaryOp::And => {
            if (lhs != 0) && (rhs != 0) {
                1
            } else {
                0
            }
        }
        BinaryOp::Or => {
            if (lhs != 0) || (rhs != 0) {
                1
            } else {
                0
            }
        }
    }
}

/// Returns the list of static functions with their types and parameter types.
fn static_functions() -> Vec<(&'static str, Type, Vec<BType>)> {
    let i32_type = Type::BType(BType::I32);
    let void_type = Type::BType(BType::Void);
    vec![
        ("getint", i32_type.clone(), vec![]),
        ("getch", i32_type.clone(), vec![]),
        (
            "getarray",
            i32_type.clone(),
            vec![BType::Ptr(Box::new(BType::I32))],
        ),
        ("putint", void_type.clone(), vec![BType::I32]),
        ("putch", void_type.clone(), vec![BType::I32]),
        (
            "putarray",
            void_type.clone(),
            vec![BType::I32, BType::Ptr(Box::new(BType::I32))],
        ),
        ("starttime", i32_type.clone(), vec![]),
        ("stoptime", i32_type.clone(), vec![]),
    ]
}

fn resolve_func_param_btype(param: &FuncParam, bg: &Background) -> BType {
    if let Some(dims) = &param.array_dims {
        let dims = dims
            .iter()
            .map(|dim| eval_param_dim(dim, &|name| bg.try_get_constant(name.to_string())))
            .collect::<Vec<_>>();
        func_param_btype(param.btype.clone(), dims)
    } else {
        param.btype.clone()
    }
}

pub fn resolve_decl_value_type(base: &Type, var: &VarDecl, bg: &Background) -> ValueType {
    let base_ty = match base {
        Type::BType(btype) | Type::Const(btype) => ValueType::from_btype(btype),
    };
    fn inner_decl(var: &VarDecl, base_ty: ValueType, bg: &Background) -> Option<ValueType> {
        match var {
            VarDecl::Ident(_) => Some(base_ty),
            VarDecl::Array(inner, len) => {
                let len = len.try_eval_const(bg)? as usize;
                let inner_ty = inner_decl(inner, base_ty, bg)?;
                Some(ValueType::Array(len, Box::new(inner_ty)))
            }
        }
    }
    inner_decl(var, base_ty, bg).expect("Array size must be a constant expression")
}

pub fn scan_global_symbol(comp_unit: CompUnit, bg: &mut Background) {
    for glob_def in &comp_unit.global_defs {
        if let GlobalDef::StructDef(def) = glob_def {
            bg.register_struct_def(def.clone());
        }
    }
    bg.resolve_struct_layouts();

    for glob_def in comp_unit.global_defs {
        match glob_def {
            GlobalDef::FuncDef(func_def) => {
                let func_name = func_def.ident.clone();
                let params: Vec<BType> = func_def
                    .func_params
                    .iter()
                    .map(|param| resolve_func_param_btype(param, bg))
                    .collect();
                if bg.global_symbols.global_function.contains_key(&func_name) {
                    panic!("Duplicate function definition: {}", func_name);
                }
                bg.global_symbols
                    .global_function
                    .insert(func_name, (func_def.func_type, params));
                bg.global_symbols
                    .async_function
                    .insert(func_def.ident, func_def.is_async);
            }
            GlobalDef::GlobalDecl(ty, decls) => {
                for decl in decls {
                    if matches!(ty, Type::Const(_)) {
                        if let (VarDecl::Ident(var_name), Some(crate::lalr::InitVal::Exp(exp))) =
                            (&decl.var, &decl.init)
                        {
                            if let Some(value) = exp.try_eval_const(bg) {
                                bg.set_constant(var_name.clone(), value);
                            }
                        }
                    }
                    let var = decl.var.clone();
                    let value_type = resolve_decl_value_type(&ty, &var, bg);
                    let var_name = match var.clone() {
                        VarDecl::Ident(name) => name,
                        VarDecl::Array(_, _) => flatten_var_decl(&var, bg)
                            .map(|(name, _)| name)
                            .expect("Array size must be a constant expression"),
                    };
                    let is_scalar_const =
                        matches!(ty, Type::Const(_)) && matches!(value_type, ValueType::Int);
                    if is_scalar_const {
                        continue;
                    }
                    if bg.global_symbols.global_variable.contains_key(&var_name) {
                        panic!("Duplicate global variable definition: {}", var_name);
                    }
                    bg.global_symbols
                        .global_variable
                        .insert(var_name, value_type);
                }
            }
            GlobalDef::StructDef(_) => {}
        }
    }

    for func_def in static_functions() {
        let func_name = func_def.0.to_string();
        let func_type = func_def.1.clone();
        let params = func_def.2.clone();
        bg.global_symbols
            .static_function
            .insert(func_name, (func_type, params));
    }
}

fn flatten_var_decl(var: &VarDecl, bg: &Background) -> Option<(String, Vec<usize>)> {
    fn inner(var: &VarDecl, bg: &Background, dims: &mut Vec<usize>) -> Option<String> {
        match var {
            VarDecl::Ident(name) => Some(name.clone()),
            VarDecl::Array(base, len) => {
                let name = inner(base, bg, dims)?;
                let len = len.try_eval_const(bg)? as usize;
                dims.push(len);
                Some(name)
            }
        }
    }

    let mut dims = Vec::new();
    let name = inner(var, bg, &mut dims)?;
    Some((name, dims))
}
