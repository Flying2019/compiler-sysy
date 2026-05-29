use clap::{Parser, ValueEnum};
use compile_sysy::asm;
use compile_sysy::asm::program_to_asm;
use compile_sysy::ast::*;
use compile_sysy::ast_tool;
use compile_sysy::ast_tool::{scan_global_symbol, Background};
use compile_sysy::koopa::KoopaLines;
use compile_sysy::lalr::CompUnit;
use compile_sysy::lower::lower_lines;
use lalrpop_util::lalrpop_mod;
use std::env;
use std::io::Result;
use std::{fs::read_to_string, path::PathBuf};

#[derive(Clone)]
struct LexToken {
    text: String,
    is_ident: bool,
    is_ws: bool,
}

#[derive(ValueEnum, Clone, Debug)]
enum Mode {
    Koopa,
    Riscv,
}

#[derive(Parser, Debug)]
#[command(version, long_about = None)]
struct Args {
    #[arg(short, long, value_enum)]
    mode: Mode,

    input: PathBuf,

    #[arg(short, long)]
    output: PathBuf,
}

lalrpop_mod!(sysy);

// 为了支持非标准的 -koopa 和 -riscv 形式进行的预处理
fn preprocess_args() -> Vec<String> {
    let mut raw_args: Vec<String> = env::args().collect();
    for arg in raw_args.iter_mut() {
        if arg == "-koopa" {
            *arg = "--mode=koopa".to_string();
        } else if arg == "-riscv" {
            *arg = "--mode=riscv".to_string();
        }
    }
    raw_args
}

fn str_to_ast(input: &str) -> CompUnit {
    let input = normalize_struct_syntax(input);
    sysy::CompUnitParser::new()
        .parse(&input)
        .expect("Failed to parse input")
}

fn normalize_struct_syntax(input: &str) -> String {
    fn is_ident_start(ch: char) -> bool {
        ch == '_' || ch.is_ascii_alphabetic()
    }
    fn is_ident_char(ch: char) -> bool {
        ch == '_' || ch.is_ascii_alphanumeric()
    }
    fn tokenize(input: &str) -> Vec<LexToken> {
        let chars: Vec<char> = input.chars().collect();
        let mut i = 0;
        let mut tokens = Vec::new();
        while i < chars.len() {
            let ch = chars[i];
            if ch.is_whitespace() {
                let start = i;
                i += 1;
                while i < chars.len() && chars[i].is_whitespace() {
                    i += 1;
                }
                tokens.push(LexToken {
                    text: chars[start..i].iter().collect(),
                    is_ident: false,
                    is_ws: true,
                });
            } else if is_ident_start(ch) {
                let start = i;
                i += 1;
                while i < chars.len() && is_ident_char(chars[i]) {
                    i += 1;
                }
                tokens.push(LexToken {
                    text: chars[start..i].iter().collect(),
                    is_ident: true,
                    is_ws: false,
                });
            } else if ch == '-' && i + 1 < chars.len() && chars[i + 1] == '>' {
                tokens.push(LexToken {
                    text: "->".to_string(),
                    is_ident: false,
                    is_ws: false,
                });
                i += 2;
            } else {
                tokens.push(LexToken {
                    text: ch.to_string(),
                    is_ident: false,
                    is_ws: false,
                });
                i += 1;
            }
        }
        tokens
    }
    fn prev_sig(tokens: &[LexToken], idx: usize) -> Option<&str> {
        tokens[..idx]
            .iter()
            .rev()
            .find(|t| !t.is_ws)
            .map(|t| t.text.as_str())
    }
    fn next_sig(tokens: &[LexToken], idx: usize) -> Option<&str> {
        tokens[idx + 1..]
            .iter()
            .find(|t| !t.is_ws)
            .map(|t| t.text.as_str())
    }
    fn next_sig_index(tokens: &[LexToken], idx: usize) -> Option<usize> {
        ((idx + 1)..tokens.len()).find(|&j| !tokens[j].is_ws)
    }

    let tokens = tokenize(input);
    let mut struct_names = std::collections::HashSet::new();
    for i in 0..tokens.len() {
        if tokens[i].text == "struct" {
            if let Some(name_idx) = next_sig_index(&tokens, i) {
                if tokens[name_idx].is_ident {
                    if let Some(brace_idx) = next_sig_index(&tokens, name_idx) {
                        if tokens[brace_idx].text == "{" {
                            struct_names.insert(tokens[name_idx].text.clone());
                        }
                    }
                }
            }
        }
    }

    let mut out = String::new();
    for i in 0..tokens.len() {
        let tok = &tokens[i];
        let prev = prev_sig(&tokens, i);
        let next = next_sig(&tokens, i);
        let next_idx = next_sig_index(&tokens, i);
        let should_prefix_struct = tok.is_ident
            && struct_names.contains(&tok.text)
            && prev != Some("struct")
            && prev != Some(".")
            && prev != Some("->")
            && match prev {
                Some("new") => true,
                Some("{") | Some(";") | Some(",") | Some("(") | Some("const") | None => {
                    matches!(next, Some("*")) || next_idx.map(|j| tokens[j].is_ident).unwrap_or(false)
                }
                _ => false,
            };
        if should_prefix_struct {
            out.push_str("struct ");
        }
        out.push_str(&tok.text);
    }
    out
}

fn ast_to_koopa_lines(ast: &CompUnit) -> (KoopaLines, Background) {
    let mut bg = ast_tool::Background::new();
    scan_global_symbol(ast.clone(), &mut bg);
    let lines = ast.to_koopa_lines(&mut bg);
    (lines, bg)
}

fn ir_to_asm(koopa_ir: &str) -> String {
    let driver = koopa::front::Driver::from(koopa_ir.to_string());
    let program = driver.generate_program().unwrap();
    let asm = program_to_asm(&program, &asm::Background::new());
    asm.to_string()
}

fn main() -> Result<()> {
    let args = Args::parse_from(preprocess_args());
    match args.mode {
        Mode::Koopa => {
            let input = read_to_string(args.input)?;
            let ast = str_to_ast(&input);
            let (koopa_lines, _) = ast_to_koopa_lines(&ast);
            std::fs::write(args.output, koopa_lines.to_wrapped_string())?;
        }
        Mode::Riscv => {
            let input = read_to_string(args.input)?;
            let ast = str_to_ast(&input);
            let (koopa_lines, bg) = ast_to_koopa_lines(&ast);
            let koopa_ir = lower_lines(&koopa_lines, &bg);
            println!("Generated Koopa IR:\n{}", koopa_lines.to_wrapped_string());
            let asm = ir_to_asm(&koopa_ir);
            std::fs::write(args.output, asm.to_string())?;
        }
    }
    Ok(())
}
