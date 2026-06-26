# official_core Coverage

This directory is the local official-style SysY core subset used when the real
official functional/performance suite is not present in the workspace.

The full selftest tree also covers compiler extensions such as struct, pointer,
Promise, async, diagnostics, and runner behavior. This directory intentionally
uses only baseline SysY syntax.

## Covered Core Areas

- integer literals and expression precedence: `expr_precedence_literals`
- dangling `else` binding and nested conditionals: `dangling_else`
- scalar function parameters and block scope shadowing: `function_params_and_scope`
- recursion and local shadowing: `recursion_shadow`
- function definition order and cross calls: `mutual_recursion`
- multi-dimensional array parameters: `multidim_param_3d`
- partial global/local array initialization: `array_partial_init_sum`
- short-circuit side effects: `short_circuit_side_effect`
- `while`, `break`, and `continue`: `loop_break_continue_mix`
- global consts, local consts, and const shadowing in array dimensions:
  `const_shadow_global`

## Related Selftest Coverage Outside This Directory

- builtin input/output: `builtin_io/get_put_array`
- C runner smoke: `basic_c/test`
- array decay and parameter edge cases: `array_param/*`
- additional initializer brace alignment: `array_init/*`, `array_init_edges/*`
- generic control-flow smoke: `control_flow/*`
- semantic and lexer diagnostics: `diagnostics/*`

If a real official SysY suite is later vendored, keep this directory as a fast
smoke subset and add the official suite under a separate directory so both can
run through `toolchain/riscv64/run_selftests.sh`.
