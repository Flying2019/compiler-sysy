use lalrpop_util::lalrpop_mod;
use compile_sysy::ast::astnode::*;
use std::env::args;
use std::fs::read_to_string;
use std::io::Result;

// 引用 lalrpop 生成的解析器
// 因为我们刚刚创建了 sysy.lalrpop, 所以模块名是 sysy
lalrpop_mod!(sysy);

fn main() -> Result<()> {
    let mut args = args();
    args.next();
    let mode = args.next().unwrap();
    let input = args.next().unwrap();
    args.next();
    let output = args.next().unwrap();
    let input = read_to_string(input)?;

    let ast = sysy::CompUnitParser::new().parse(&input).unwrap();

    let ir = ast.to_koopa_ir();
    std::fs::write(output, ir)?;
    Ok(())
}
