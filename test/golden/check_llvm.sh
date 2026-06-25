#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

check_case() {
  local name="$1"
  local source="$2"
  local patterns="$3"
  local output="$TMPDIR/$name.ll"

  cargo run --quiet --manifest-path "$ROOT_DIR/sysy/Cargo.toml" -- \
    -llvm --output "$output" "$ROOT_DIR/$source"

  while IFS= read -r pattern; do
    if [[ -z "$pattern" ]]; then
      continue
    fi
    if ! grep -Fq "$pattern" "$output"; then
      printf 'golden llvm %s: missing pattern: %s\n' "$name" "$pattern" >&2
      exit 1
    fi
  done < "$ROOT_DIR/$patterns"

  if grep -q '__sysy_async_step_' "$output" || grep -q 'async\.state\.' "$output"; then
    printf 'golden llvm %s: obsolete async state-machine lowering was emitted\n' "$name" >&2
    exit 1
  fi
  if grep -q '__state' "$output"; then
    printf 'golden llvm %s: obsolete async frame state field was emitted\n' "$name" >&2
    exit 1
  fi
  if grep -q 'getelementptr i8, ptr %frame' "$output"; then
    printf 'golden llvm %s: async frame uses raw byte-pointer GEP\n' "$name" >&2
    exit 1
  fi
  if grep -qi 'koopa' "$output"; then
    printf 'golden llvm %s: legacy Koopa marker was emitted\n' "$name" >&2
    exit 1
  fi

  printf 'ok golden llvm %s\n' "$name"
}

check_case \
  async \
  sysy/test/sample.sysy \
  sysy/test/golden/llvm/async.patterns

check_case \
  struct \
  sysy/test/selftest/struct_nested/pointer_chain.sysy \
  sysy/test/golden/llvm/struct.patterns
