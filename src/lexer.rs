use std::collections::HashSet;

pub type Spanned<Tok, Loc, Error> = Result<(Loc, Tok, Loc), Error>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LexError {
    InvalidChar(char),
    UnterminatedBlockComment,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tok {
    Ident(String),
    TypeIdent(String),
    IntConst(i32),
    Int,
    Void,
    Const,
    Struct,
    If,
    Else,
    While,
    Continue,
    Break,
    Return,
    New,
    Async,
    Await,
    Promise,
    Sleep,
    Wait,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Bang,
    Amp,
    AmpAmp,
    PipePipe,
    Eq,
    EqEq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Assign,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semicolon,
    Dot,
    Arrow,
}

pub struct Lexer<'input> {
    input: &'input str,
    chars: Vec<(usize, char)>,
    pos: usize,
    struct_names: HashSet<String>,
    expect_struct_name: bool,
    possible_bare_struct_name: Option<String>,
}

impl<'input> Lexer<'input> {
    pub fn new(input: &'input str) -> Self {
        Self {
            input,
            chars: input.char_indices().collect(),
            pos: 0,
            struct_names: HashSet::new(),
            expect_struct_name: false,
            possible_bare_struct_name: None,
        }
    }

    fn byte_pos(&self) -> usize {
        self.chars
            .get(self.pos)
            .map(|(idx, _)| *idx)
            .unwrap_or(self.input.len())
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).map(|(_, ch)| *ch)
    }

    fn peek_next(&self) -> Option<char> {
        self.chars.get(self.pos + 1).map(|(_, ch)| *ch)
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += 1;
        Some(ch)
    }

    fn skip_ws_and_comments(&mut self) -> Result<(), LexError> {
        loop {
            while self.peek().is_some_and(char::is_whitespace) {
                self.bump();
            }
            if self.peek() == Some('/') && self.peek_next() == Some('/') {
                while let Some(ch) = self.bump() {
                    if ch == '\n' || ch == '\r' {
                        break;
                    }
                }
                continue;
            }
            if self.peek() == Some('/') && self.peek_next() == Some('*') {
                self.bump();
                self.bump();
                loop {
                    match (self.peek(), self.peek_next()) {
                        (Some('*'), Some('/')) => {
                            self.bump();
                            self.bump();
                            break;
                        }
                        (Some(_), _) => {
                            self.bump();
                        }
                        (None, _) => return Err(LexError::UnterminatedBlockComment),
                    }
                }
                continue;
            }
            return Ok(());
        }
    }

    fn is_ident_start(ch: char) -> bool {
        ch == '_' || ch.is_ascii_alphabetic()
    }

    fn is_ident_char(ch: char) -> bool {
        ch == '_' || ch.is_ascii_alphanumeric()
    }

    fn take_ident(&mut self) -> String {
        let start = self.pos;
        self.bump();
        while self.peek().is_some_and(Self::is_ident_char) {
            self.bump();
        }
        self.chars[start..self.pos]
            .iter()
            .map(|(_, ch)| *ch)
            .collect()
    }

    fn take_number(&mut self) -> String {
        let start = self.pos;
        self.bump();
        while self
            .peek()
            .is_some_and(|ch| ch.is_ascii_hexdigit() || ch == 'x' || ch == 'X')
        {
            self.bump();
        }
        self.chars[start..self.pos]
            .iter()
            .map(|(_, ch)| *ch)
            .collect()
    }

    fn parse_int(text: &str) -> i32 {
        if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
            i32::from_str_radix(hex, 16).unwrap()
        } else if text.len() > 1 && text.starts_with('0') {
            i32::from_str_radix(text, 8).unwrap()
        } else {
            text.parse().unwrap()
        }
    }

    fn token(&mut self, tok: Tok) -> Tok {
        match &tok {
            Tok::Struct => {
                self.expect_struct_name = true;
                self.possible_bare_struct_name = None;
            }
            Tok::LBrace => {
                if let Some(name) = self.possible_bare_struct_name.take() {
                    self.struct_names.insert(name);
                }
            }
            Tok::Semicolon
            | Tok::RBrace
            | Tok::LParen
            | Tok::RParen
            | Tok::LBracket
            | Tok::Assign
            | Tok::Comma => {
                self.possible_bare_struct_name = None;
            }
            _ => {}
        }
        tok
    }
}

impl Iterator for Lexer<'_> {
    type Item = Spanned<Tok, usize, LexError>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Err(err) = self.skip_ws_and_comments() {
            return Some(Err(err));
        }
        let start = self.byte_pos();
        let ch = self.peek()?;
        let tok = if Self::is_ident_start(ch) {
            let ident = self.take_ident();
            match ident.as_str() {
                "int" => self.token(Tok::Int),
                "void" => self.token(Tok::Void),
                "const" => self.token(Tok::Const),
                "struct" => self.token(Tok::Struct),
                "if" => self.token(Tok::If),
                "else" => self.token(Tok::Else),
                "while" => self.token(Tok::While),
                "continue" => self.token(Tok::Continue),
                "break" => self.token(Tok::Break),
                "return" => self.token(Tok::Return),
                "new" => self.token(Tok::New),
                "async" => self.token(Tok::Async),
                "await" => self.token(Tok::Await),
                "Promise" => self.token(Tok::Promise),
                "sleep" => self.token(Tok::Sleep),
                "wait" => self.token(Tok::Wait),
                _ => {
                    if self.expect_struct_name {
                        self.expect_struct_name = false;
                        self.struct_names.insert(ident.clone());
                        self.possible_bare_struct_name = Some(ident.clone());
                        Tok::TypeIdent(ident)
                    } else if self.struct_names.contains(&ident) {
                        self.possible_bare_struct_name = Some(ident.clone());
                        Tok::TypeIdent(ident)
                    } else {
                        self.possible_bare_struct_name = Some(ident.clone());
                        Tok::Ident(ident)
                    }
                }
            }
        } else if ch.is_ascii_digit() {
            let number = self.take_number();
            self.token(Tok::IntConst(Self::parse_int(&number)))
        } else {
            self.bump();
            let tok = match ch {
                '+' => Tok::Plus,
                '-' if self.peek() == Some('>') => {
                    self.bump();
                    Tok::Arrow
                }
                '-' => Tok::Minus,
                '*' => Tok::Star,
                '/' => Tok::Slash,
                '%' => Tok::Percent,
                '!' if self.peek() == Some('=') => {
                    self.bump();
                    Tok::Ne
                }
                '!' => Tok::Bang,
                '&' if self.peek() == Some('&') => {
                    self.bump();
                    Tok::AmpAmp
                }
                '&' => Tok::Amp,
                '|' if self.peek() == Some('|') => {
                    self.bump();
                    Tok::PipePipe
                }
                '=' if self.peek() == Some('=') => {
                    self.bump();
                    Tok::EqEq
                }
                '=' => Tok::Assign,
                '<' if self.peek() == Some('=') => {
                    self.bump();
                    Tok::Le
                }
                '<' => Tok::Lt,
                '>' if self.peek() == Some('=') => {
                    self.bump();
                    Tok::Ge
                }
                '>' => Tok::Gt,
                '(' => Tok::LParen,
                ')' => Tok::RParen,
                '{' => Tok::LBrace,
                '}' => Tok::RBrace,
                '[' => Tok::LBracket,
                ']' => Tok::RBracket,
                ',' => Tok::Comma,
                ';' => Tok::Semicolon,
                '.' => Tok::Dot,
                other => return Some(Err(LexError::InvalidChar(other))),
            };
            self.token(tok)
        };
        let end = self.byte_pos();
        Some(Ok((start, tok, end)))
    }
}
