use crate::ast_tool::{
    eval_binary_const, eval_unary_const, gen_binary_koopa_ir, gen_unary_koopa_ir, Background,
    ValueType,
};
use crate::koopa::{KoopaLine, KoopaLines};
use crate::lalr::*;
use std::fmt::Debug;

#[derive(Debug, Clone)]
pub struct ReturnValue {
    pub value: Option<String>,
    pub content: KoopaLines,
}

impl ReturnValue {
    pub fn new(value: Option<String>, content: KoopaLines) -> Self {
        Self { value, content }
    }
    pub fn with_content(content: KoopaLines) -> Self {
        Self {
            value: None,
            content,
        }
    }
    pub fn extend(mut self, additional: ReturnValue) -> Self {
        self.content.add_lines(additional.content);
        self.value = additional.value;
        self
    }
}

#[allow(unused)]
pub trait AstNode: Debug {
    fn to_koopa(&self, bg: &mut Background) -> ReturnValue {
        ReturnValue::with_content(KoopaLines::new())
    }
    fn to_koopa_lines(&self, bg: &mut Background) -> KoopaLines {
        self.to_koopa(bg).content
    }
}

#[derive(Debug, Clone)]
struct LValue {
    ptr: String,
    ty: ValueType,
    content: KoopaLines,
}

fn product(dims: &[usize]) -> usize {
    dims.iter().product()
}

fn flatten_var_decl(var: &VarDecl, bg: &Background) -> (String, Vec<usize>) {
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
    let name = inner(var, bg, &mut dims).expect("Array size must be a constant expression");
    (name, dims)
}

fn value_type_from_decl(var: &VarDecl, bg: &Background) -> ValueType {
    match var {
        VarDecl::Ident(_) => ValueType::Int,
        VarDecl::Array(_, _) => ValueType::from_dims(&flatten_var_decl(var, bg).1),
    }
}

fn value_type_dims(ty: &ValueType) -> Vec<usize> {
    match ty {
        ValueType::Int | ValueType::Pointer(_) => Vec::new(),
        ValueType::Array(len, inner) => {
            let mut dims = vec![*len];
            dims.extend(value_type_dims(inner));
            dims
        }
    }
}

fn init_slots(init: &InitVal, dims: &[usize]) -> Vec<Option<InitVal>> {
    let total = if dims.is_empty() { 1 } else { product(dims) };
    let mut slots = vec![None; total];
    fill_init_slots(init, dims, 0, total, &mut slots);
    slots
}

fn fill_init_slots(
    init: &InitVal,
    dims: &[usize],
    start: usize,
    end: usize,
    slots: &mut [Option<InitVal>],
) {
    fn scalar_init(init: &InitVal) -> Option<InitVal> {
        match init {
            InitVal::Exp(_) => Some(init.clone()),
            InitVal::Arr(items) => items.first().and_then(scalar_init),
        }
    }

    if dims.is_empty() {
        if let Some(init) = scalar_init(init) {
            slots[start] = Some(init);
        }
        return;
    }

    match init {
        InitVal::Exp(_) => {
            slots[start] = Some(init.clone());
        }
        InitVal::Arr(items) => {
            let child_len = if dims.len() == 1 { 1 } else { product(&dims[1..]) };
            let mut pos = start;
            for item in items {
                if pos >= end {
                    break;
                }
                match item {
                    InitVal::Exp(_) => {
                        slots[pos] = Some(item.clone());
                        pos += 1;
                    }
                    InitVal::Arr(_) => {
                        if child_len > 1 {
                            let offset = pos - start;
                            let rem = offset % child_len;
                            if rem != 0 {
                                pos += child_len - rem;
                            }
                        }
                        if pos >= end {
                            break;
                        }
                        let next = (pos + child_len).min(end);
                        fill_init_slots(item, &dims[1..], pos, next, slots);
                        pos = next;
                    }
                }
            }
        }
    }
}

fn nested_init_string(values: &[String], dims: &[usize]) -> String {
    if dims.len() == 1 {
        return format!("{{{}}}", values.join(", "));
    }
    let child_len = product(&dims[1..]);
    let mut parts = Vec::new();
    for idx in 0..dims[0] {
        let start = idx * child_len;
        let end = start + child_len;
        parts.push(nested_init_string(&values[start..end], &dims[1..]));
    }
    format!("{{{}}}", parts.join(", "))
}

fn flat_index_to_indices(mut index: usize, dims: &[usize]) -> Vec<usize> {
    let mut indices = Vec::with_capacity(dims.len());
    let mut strides = Vec::with_capacity(dims.len());
    let mut stride = 1;
    for dim in dims.iter().rev() {
        strides.push(stride);
        stride *= *dim;
    }
    strides.reverse();
    for (dim, stride) in dims.iter().zip(strides) {
        let idx = index / stride;
        indices.push(idx);
        index %= stride;
        debug_assert!(idx < *dim);
    }
    indices
}

fn gen_array_elem_ptr(
    bg: &mut Background,
    base_ptr: String,
    ty: &ValueType,
    indices: &[usize],
) -> ReturnValue {
    let mut ir = KoopaLines::new();
    let mut ptr = base_ptr;
    let mut current_ty = ty.clone();
    for &idx in indices {
        match current_ty.clone() {
            ValueType::Array(_, elem_ty) => {
                let slot_ptr = bg.next_temp();
                ir.add_line(KoopaLine::GetElemPtr(
                    slot_ptr.clone(),
                    ptr,
                    idx.to_string(),
                ));
                ptr = slot_ptr;
                current_ty = (*elem_ty).clone();
            }
            ValueType::Pointer(inner_ty) => {
                let loaded_ptr = bg.next_temp();
                ir.add_line(KoopaLine::Load(loaded_ptr.clone(), ptr));
                let next_ptr = bg.next_temp();
                ir.add_line(KoopaLine::GetPtr(
                    next_ptr.clone(),
                    loaded_ptr,
                    idx.to_string(),
                ));
                ptr = next_ptr;
                current_ty = (*inner_ty).clone();
            }
            ValueType::Int => panic!("Cannot index into int"),
        }
    }
    ReturnValue::new(Some(ptr), ir)
}

fn gen_lvalue(exp: &Exp, bg: &mut Background) -> LValue {
    match exp {
        Exp::Ident(name) => LValue {
            ptr: bg.get_variable(name.clone()),
            ty: bg.get_variable_type(name.clone()),
            content: KoopaLines::new(),
        },
        Exp::ArrGet(arr, idx) => {
            let mut base = gen_lvalue(arr, bg);
            let idx_ret = idx.to_koopa(bg);
            base.content.add_lines(idx_ret.content);
            match base.ty.clone() {
                ValueType::Int => panic!("Int value cannot be indexed"),
                ValueType::Array(_, elem_ty) => {
                    let ptr = bg.next_temp();
                    base.content.add_line(KoopaLine::GetElemPtr(
                        ptr.clone(),
                        base.ptr,
                        idx_ret.value.unwrap(),
                    ));
                    LValue {
                        ptr,
                        ty: (*elem_ty).clone(),
                        content: base.content,
                    }
                }
                ValueType::Pointer(inner_ty) => {
                    let loaded_ptr = bg.next_temp();
                    base.content
                        .add_line(KoopaLine::Load(loaded_ptr.clone(), base.ptr));
                    let ptr = bg.next_temp();
                    base.content.add_line(KoopaLine::GetPtr(
                        ptr.clone(),
                        loaded_ptr,
                        idx_ret.value.unwrap(),
                    ));
                    LValue {
                        ptr,
                        ty: (*inner_ty).clone(),
                        content: base.content,
                    }
                }
            }
        }
        _ => panic!("Expression {:?} is not an lvalue", exp),
    }
}

impl AstNode for CompUnit {
    fn to_koopa_lines(&self, bg: &mut Background) -> KoopaLines {
        if bg.get_function("main".to_string()).is_none() {
            panic!("No main function defined, or global symbol scanner didn't run");
        }
        let mut lines = KoopaLines::new();
        for func in bg.get_static_functions_decl() {
            let name = func.0;
            let params = func
                .2
                .iter()
                .map(|b| b.to_koopa_type())
                .collect::<Vec<_>>()
                .join(", ");
            let ret = func.1;
            lines.add_line(KoopaLine::FuncDecl(name, params, ret.to_koopa_type()));
        }
        for glob_def in &self.glob_defs {
            lines.add_lines(glob_def.to_koopa_lines(bg));
        }
        lines
    }
}

impl AstNode for GlobleDef {
    fn to_koopa_lines(&self, bg: &mut Background) -> KoopaLines {
        match self {
            GlobleDef::FuncDef(func_def) => func_def.to_koopa_lines(bg),
            GlobleDef::GlobleDecl(typ, decls) => {
                let mut lines = KoopaLines::new();
                for decl in decls {
                    let value_type = value_type_from_decl(&decl.var, bg);
                    if matches!(typ, Type::Const(_)) && matches!(value_type, ValueType::Int) {
                        if let Some(init) = &decl.init {
                            let value = init.try_eval_const(bg).expect(
                                "Const variable must be initialized with a constant expression",
                            );
                            match decl.var.clone() {
                                VarDecl::Ident(var_name) => {
                                    if bg.try_get_constant(var_name.clone()).is_none() {
                                        bg.set_constant(var_name, value);
                                    }
                                }
                                _ => unreachable!(),
                            }
                        } else {
                            panic!("Const variable {:?} must be initialized", decl.var);
                        }
                        continue;
                    }

                    match value_type {
                        ValueType::Int => {
                            let init_value = if let Some(init) = &decl.init {
                                let value = init.try_eval_const(bg).expect(
                                    "Global variable must be initialized with a constant expression",
                                );
                                value.to_string()
                            } else {
                                "zeroinit".to_string()
                            };
                            if let VarDecl::Ident(var_name) = &decl.var {
                                let new_name = bg.new_variable(var_name.clone());
                                bg.set_variable_type(new_name.clone(), ValueType::Int);
                                lines.add_line(KoopaLine::GlobalAlloc(
                                    new_name,
                                    ValueType::Int.to_koopa_type(),
                                    init_value,
                                ));
                            }
                        }
                        array_ty @ ValueType::Array(_, _) => {
                            let dims = value_type_dims(&array_ty);
                            let (var_name, _) = flatten_var_decl(&decl.var, bg);
                            let new_name = bg.new_variable(var_name);
                            bg.set_variable_type(new_name.clone(), array_ty.clone());
                            let init_value = if let Some(init) = &decl.init {
                                let values = init_slots(init, &dims)
                                    .into_iter()
                                    .map(|slot| match slot {
                                        Some(init) => init
                                            .try_eval_const(bg)
                                            .expect("Global array initializer must be constant")
                                            .to_string(),
                                        None => "0".to_string(),
                                    })
                                    .collect::<Vec<_>>();
                                nested_init_string(&values, &dims)
                            } else {
                                "zeroinit".to_string()
                            };
                            lines.add_line(KoopaLine::GlobalAlloc(
                                new_name,
                                array_ty.to_koopa_type(),
                                init_value,
                            ));
                        }
                        ValueType::Pointer(_) => unreachable!("Variables are never declared as raw pointers here"),
                    }
                }
                lines
            }
        }
    }
}

impl AstNode for FuncDef {
    fn to_koopa_lines(&self, bg: &mut Background) -> KoopaLines {
        bg.clear();
        let record = bg.record();
        bg.increment_time();
        let mut lines = KoopaLines::new();
        let params = self
            .func_params
            .iter()
            .map(FuncParam::to_koopa_param)
            .collect::<Vec<_>>()
            .join(", ");
        lines.add_line(KoopaLine::FuncStart(
            self.ident.clone(),
            params,
            self.func_type.to_koopa_type(),
        ));
        lines.add_line(KoopaLine::Label("%entry".to_string()));
        for param in &self.func_params {
            let ptr_name = bg.new_variable(param.name.clone());
            lines.add_line(KoopaLine::Alloc(ptr_name.clone(), param.to_koopa_type()));
            lines.add_line(KoopaLine::Store(param.to_koopa_name(), ptr_name.clone()));
            bg.set_variable_type(ptr_name, ValueType::from_btype(&param.btype));
        }
        lines.add_lines(self.block.to_koopa_lines(bg));
        lines.close();
        bg.rollback(record);
        lines
    }
}

impl FuncParam {
    fn to_koopa_type(&self) -> String {
        self.btype.to_koopa_type()
    }
    fn to_koopa_name(&self) -> String {
        format!("@arg_{}", self.name)
    }
    fn to_koopa_param(&self) -> String {
        format!("{}: {}", self.to_koopa_name(), self.to_koopa_type())
    }
}

impl BType {
    fn to_koopa_type(&self) -> String {
        match self {
            BType::I32 => "i32".to_string(),
            BType::Void => "void".to_string(),
            BType::Ptr(b) => format!("*{}", b.to_koopa_type()),
            BType::Array(len, b) => format!("[{}, {}]", b.to_koopa_type(), len),
        }
    }
}

impl Type {
    fn to_koopa_type(&self) -> String {
        match self {
            Type::BType(b) => b.to_koopa_type(),
            Type::Const(b) => b.to_koopa_type(),
        }
    }
}

impl AstNode for Vec<Stmt> {
    fn to_koopa_lines(&self, bg: &mut Background) -> KoopaLines {
        let ts = bg.record();
        bg.increment_time();
        let mut block_lines = KoopaLines::new();
        let next_bb = bg.next_bb().clone();
        // Only the last statement in the block set bb to next_bb. Others set to None.
        for (id, stmt) in self.iter().enumerate() {
            if id == self.len() - 1 && next_bb.is_some() {
                bg.set_next_bb(next_bb.clone());
                block_lines.add_lines(stmt.to_koopa_lines(bg));
            } else {
                bg.set_next_bb(None);
                block_lines.add_lines(stmt.to_koopa_lines(bg));
                if let Some(bb) = bg.next_bb().clone() {
                    block_lines.add_line(KoopaLine::Label(bb));
                }
            }
        }
        bg.rollback(ts);
        bg.set_next_bb(next_bb);
        block_lines
    }
}

impl AstNode for Stmt {
    fn to_koopa_lines(&self, bg: &mut Background) -> KoopaLines {
        // next_bb = None means the next stmts hasn't closed yet.
        match self {
            Stmt::Block(block) => block.to_koopa_lines(bg),
            Stmt::Assign(left, right) => {
                let exp_ret = right.to_koopa(bg);
                let lvalue = gen_lvalue(left, bg);
                let mut content = lvalue.content;
                content.add_lines(exp_ret.content);
                content.add_line(KoopaLine::Store(exp_ret.value.unwrap(), lvalue.ptr));
                content
            }
            Stmt::Decl(typ, decls) => {
                let mut content = KoopaLines::new();
                for decl in decls {
                    let value_type = value_type_from_decl(&decl.var, bg);
                    if matches!(typ, Type::Const(_)) && matches!(value_type, ValueType::Int) {
                        if let Some(init) = &decl.init {
                            let value = init.try_eval_const(bg).expect(
                                "Const variable must be initialized with a constant expression",
                            );
                            if let VarDecl::Ident(var_name) = decl.var.clone() {
                                bg.set_constant(var_name, value);
                            }
                        } else {
                            panic!("Const variable {:?} must be initialized", decl.var);
                        }
                        continue;
                    }

                    match value_type {
                        ValueType::Int => {
                            if let VarDecl::Ident(var_name) = &decl.var {
                                let ptr_name = bg.new_variable(var_name.clone());
                                bg.set_variable_type(ptr_name.clone(), ValueType::Int);
                                content.add_line(KoopaLine::Alloc(
                                    ptr_name.clone(),
                                    ValueType::Int.to_koopa_type(),
                                ));
                                if let Some(init) = &decl.init {
                                    let init_ret = init.to_koopa(bg);
                                    content.add_lines(init_ret.content);
                                    content.add_line(KoopaLine::Store(
                                        init_ret.value.unwrap(),
                                        ptr_name,
                                    ));
                                }
                            }
                        }
                        array_ty @ ValueType::Array(_, _) => {
                            let dims = value_type_dims(&array_ty);
                            let (var_name, _) = flatten_var_decl(&decl.var, bg);
                            let ptr_name = bg.new_variable(var_name);
                            bg.set_variable_type(ptr_name.clone(), array_ty.clone());
                            content.add_line(KoopaLine::Alloc(ptr_name.clone(), array_ty.to_koopa_type()));
                            if let Some(init) = &decl.init {
                                for (flat_idx, slot) in init_slots(init, &dims).into_iter().enumerate() {
                                    let value_ret = match slot {
                                        Some(init) => init.to_koopa(bg),
                                        None => ReturnValue::new(Some("0".to_string()), KoopaLines::new()),
                                    };
                                    let indices = flat_index_to_indices(flat_idx, &dims);
                                    let ptr_ret =
                                        gen_array_elem_ptr(bg, ptr_name.clone(), &array_ty, &indices);
                                    content.add_lines(value_ret.content);
                                    content.add_lines(ptr_ret.content);
                                    content.add_line(KoopaLine::Store(
                                        value_ret.value.unwrap(),
                                        ptr_ret.value.unwrap(),
                                    ));
                                }
                            }
                        }
                        ValueType::Pointer(_) => unreachable!("Variables are never declared as raw pointers here"),
                    }
                }
                content
            }
            Stmt::If(_, _)
            | Stmt::IfElse(_, _, _)
            | Stmt::Return(_)
            | Stmt::While(_, _)
            | Stmt::Continue
            | Stmt::Break => {
                // These methods require a closed basic block.
                let mut ir = KoopaLines::new();
                bg.new_next_bb_if_none();
                match self {
                    Stmt::If(exp, stmt) => {
                        let exp_ret = exp.to_koopa(bg);
                        let cond = exp_ret.value.unwrap();
                        let bb_name = bg.new_bb();
                        let then_branch = format!("{}_then", bb_name);
                        ir.add_lines(exp_ret.content);
                        ir.add_line(KoopaLine::Br(
                            cond,
                            then_branch.clone(),
                            bg.next_bb().clone().unwrap(),
                        ));
                        // Then Block
                        ir.add_lines(KoopaLines::make_block(
                            stmt.to_koopa_lines(bg),
                            then_branch,
                            bg.next_bb().clone(),
                        ));
                    }
                    Stmt::IfElse(exp, then_stmt, else_stmt) => {
                        let exp_ret = exp.to_koopa(bg);
                        let cond = exp_ret.value.unwrap();
                        let bb_name = bg.new_bb();
                        let then_branch = format!("{}_then", bb_name);
                        let else_branch = format!("{}_else", bb_name);
                        ir.add_lines(exp_ret.content);
                        ir.add_line(KoopaLine::Br(
                            cond,
                            then_branch.clone(),
                            else_branch.clone(),
                        ));
                        ir.add_lines(KoopaLines::make_block(
                            then_stmt.to_koopa_lines(bg),
                            then_branch,
                            bg.next_bb().clone(),
                        ));
                        ir.add_lines(KoopaLines::make_block(
                            else_stmt.to_koopa_lines(bg),
                            else_branch,
                            bg.next_bb().clone(),
                        ));
                    }
                    Stmt::While(exp, stmt) => {
                        let old_entry = bg.get_loop_entry();
                        let old_next = bg.get_loop_next();
                        let bb_name = bg.new_bb();
                        let entry_bb = format!("{}_entry", bb_name);
                        let body_bb = format!("{}_body", bb_name);
                        ir.add_line(KoopaLine::Jump(entry_bb.clone()));
                        ir.add_line(KoopaLine::Label(entry_bb.clone()));
                        let exp_ret = exp.to_koopa(bg);
                        ir.add_lines(exp_ret.content);
                        bg.set_loop(bg.next_bb(), Some(entry_bb.clone()));
                        let cond = exp_ret.value.unwrap();
                        ir.add_line(KoopaLine::Br(
                            cond,
                            body_bb.clone(),
                            bg.next_bb().clone().unwrap(),
                        ));
                        let old_next_bb = bg.next_bb();
                        bg.set_next_bb(Some(entry_bb.clone()));
                        ir.add_lines(KoopaLines::make_block(
                            stmt.to_koopa_lines(bg),
                            body_bb,
                            Some(entry_bb),
                        ));
                        bg.set_next_bb(old_next_bb);
                        bg.set_loop(old_next, old_entry);
                    }
                    Stmt::Return(num) => match num {
                        Some(num) => {
                            let ret = num.to_koopa(bg);
                            ir.add_lines(ret.content);
                            ir.add_line(KoopaLine::Ret(ret.value.unwrap()));
                        }
                        None => ir.add_line(KoopaLine::VoidRet),
                    },
                    Stmt::Break => {
                        ir.add_line(KoopaLine::Jump(bg.get_loop_next().unwrap()));
                    }
                    Stmt::Continue => {
                        ir.add_line(KoopaLine::Jump(bg.get_loop_entry().unwrap()));
                    }
                    _ => unreachable!(),
                }
                ir
            }
            Stmt::Exp(exp) => {
                let exp_ret = exp.to_koopa(bg);
                exp_ret.content
            }
            Stmt::Empty => KoopaLines::new(),
        }
    }
}

impl AstNode for Exp {
    fn to_koopa(&self, bg: &mut Background) -> ReturnValue {
        match self {
            Exp::Number(num) => ReturnValue::new(Some(num.to_string()), KoopaLines::new()),
            Exp::UnaryExp(op, exp) => {
                let exp_ret = exp.to_koopa(bg);
                let src = exp_ret.value.clone().unwrap();
                exp_ret.extend(gen_unary_koopa_ir(op, src, bg))
            }
            Exp::BinaryExp(op, left, right) => gen_binary_koopa_ir(op, left, right, bg),
            Exp::Ident(name) => {
                if let Some(const_value) = bg.try_get_constant(name.clone()) {
                    ReturnValue::new(Some(const_value.to_string()), KoopaLines::new())
                } else {
                    let ptr_name = bg.get_variable(name.clone());
                    match bg.get_variable_type(name.clone()) {
                        ValueType::Int => {
                            let loaded_name = bg.next_temp();
                            let mut content = KoopaLines::new();
                            content.add_line(KoopaLine::Load(loaded_name.clone(), ptr_name));
                            ReturnValue::new(Some(loaded_name), content)
                        }
                        ValueType::Array(_, _) => {
                            let elem_ptr = bg.next_temp();
                            let mut content = KoopaLines::new();
                            content.add_line(KoopaLine::GetElemPtr(
                                elem_ptr.clone(),
                                ptr_name,
                                "0".to_string(),
                            ));
                            ReturnValue::new(Some(elem_ptr), content)
                        }
                        ValueType::Pointer(_) => {
                            let loaded_name = bg.next_temp();
                            let mut content = KoopaLines::new();
                            content.add_line(KoopaLine::Load(loaded_name.clone(), ptr_name));
                            ReturnValue::new(Some(loaded_name), content)
                        }
                    }
                }
            }
            Exp::FuncCall(name, args) => {
                let func_type = bg.get_function(name.clone()).cloned().unwrap().0;
                let mut ir = KoopaLines::new();
                let mut arg_values = Vec::new();
                for arg in args {
                    let arg_ret = arg.to_koopa(bg);
                    ir.add_lines(arg_ret.content);
                    arg_values.push(arg_ret.value.unwrap());
                }
                let args = arg_values.join(", ");
                match func_type {
                    Type::BType(func_type) => match func_type {
                        BType::Void => {
                            ir.add_line(KoopaLine::VoidCall(name.clone(), args));
                            ReturnValue {
                                value: None,
                                content: ir,
                            }
                        }
                        BType::I32 => {
                            let ret_value = bg.next_temp();
                            ir.add_line(KoopaLine::Call(ret_value.clone(), name.clone(), args));
                            ReturnValue {
                                value: Some(ret_value),
                                content: ir,
                            }
                        }
                        _ => {
                            unimplemented!("Function return type {:?} not supported yet", func_type)
                        }
                    },
                    _ => unimplemented!("Function type {:?} not supported yet", func_type),
                }
            },
            Exp::ArrGet(_, _) => {
                let lvalue = gen_lvalue(self, bg);
                match lvalue.ty {
                    ValueType::Int => {
                        let loaded = bg.next_temp();
                        let mut content = lvalue.content;
                        content.add_line(KoopaLine::Load(loaded.clone(), lvalue.ptr));
                        ReturnValue::new(Some(loaded), content)
                    }
                    ValueType::Array(_, _) => {
                        let elem_ptr = bg.next_temp();
                        let mut content = lvalue.content;
                        content.add_line(KoopaLine::GetElemPtr(
                            elem_ptr.clone(),
                            lvalue.ptr,
                            "0".to_string(),
                        ));
                        ReturnValue::new(Some(elem_ptr), content)
                    }
                    ValueType::Pointer(_) => {
                        let loaded = bg.next_temp();
                        let mut content = lvalue.content;
                        content.add_line(KoopaLine::Load(loaded.clone(), lvalue.ptr));
                        ReturnValue::new(Some(loaded), content)
                    }
                }
            }
        }
    }
}

impl Exp {
    pub(crate) fn try_eval_const(&self, bg: &Background) -> Option<i32> {
        match self {
            Exp::Number(num) => Some(*num),
            Exp::UnaryExp(op, exp) => {
                let value = exp.try_eval_const(bg)?;
                Some(eval_unary_const(op, value))
            }
            Exp::BinaryExp(op, left, right) => {
                let lhs = left.try_eval_const(bg)?;
                let rhs = right.try_eval_const(bg)?;
                Some(eval_binary_const(op, lhs, rhs))
            }
            Exp::Ident(name) => bg.try_get_constant(name.clone()),
            Exp::FuncCall(_, _) => None,
            Exp::ArrGet(_, _) => None,
        }
    }
}

impl InitVal {
    fn try_eval_const(&self, bg: &Background) -> Option<i32> {
        match self {
            InitVal::Arr(_) => None,
            InitVal::Exp(exp) => exp.try_eval_const(bg)
        }
    }
}

impl AstNode for InitVal {
    fn to_koopa(&self, bg: &mut Background) -> ReturnValue {
        match self {
            InitVal::Arr(_) => panic!("Array initializer cannot be lowered as a scalar value"),
            InitVal::Exp(exp) => exp.to_koopa(bg)
        }
    }
}
