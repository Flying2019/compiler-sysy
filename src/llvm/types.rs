use crate::lalr::BType;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LlvmType {
    I32,
    I64,
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
