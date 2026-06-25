use crate::lalr::BType;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LlvmType {
    I32,
    Void,
    Ptr(Box<LlvmType>),
    Struct(String),
    Array(usize, Box<LlvmType>),
    Promise(Box<LlvmType>),
}

impl LlvmType {
    pub(crate) fn from_btype(btype: &BType) -> Self {
        match btype {
            BType::I32 => Self::I32,
            BType::Void => Self::Void,
            BType::Struct(name) => Self::Struct(name.clone()),
            BType::Promise(inner) => Self::Promise(Box::new(Self::from_btype(inner))),
            BType::Ptr(inner) => Self::Ptr(Box::new(Self::from_btype(inner))),
            BType::Array(len, inner) => Self::Array(*len, Box::new(Self::from_btype(inner))),
        }
    }

    pub(crate) fn llvm(&self) -> String {
        match self {
            Self::I32 => "i32".to_string(),
            Self::Void => "void".to_string(),
            Self::Ptr(_) | Self::Promise(_) => "ptr".to_string(),
            Self::Struct(name) => format!("%struct.{}", sanitize_ident(name)),
            Self::Array(len, inner) => format!("[{} x {}]", len, inner.llvm()),
        }
    }

    pub(crate) fn promise_value(&self) -> LlvmType {
        match self {
            Self::Promise(inner) => (**inner).clone(),
            other => other.clone(),
        }
    }

    pub(crate) fn is_void(&self) -> bool {
        matches!(self, Self::Void)
    }
}

fn sanitize_ident(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}
