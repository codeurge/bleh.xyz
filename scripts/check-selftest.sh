#!/usr/bin/env bash
# Self-test for scripts/check.sh.
#
# Copies the current working tree into a temporary clone, confirms the clean
# tree passes, then creates one violation at a time and asserts that
# check.sh fails with the expected message. Violating strings are assembled
# at run time so this file itself passes the checks.
#
#   scripts/check-selftest.sh

set -euo pipefail
src=$(git rev-parse --show-toplevel)
tmp=$(mktemp -d "${TMPDIR:-/tmp}/check-selftest.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
repo="$tmp/repo"
out="$tmp/out"

git clone -q --no-hardlinks "$src" "$repo"
# Mirror tracked and untracked (not ignored) files from the working tree.
(
  cd "$src"
  git ls-files -z --cached --others --exclude-standard \
    | while IFS= read -r -d '' f; do [[ -e "$f" ]] && printf '%s\0' "$f"; done \
    | tar --null -T - -cf -
) | tar -xf - -C "$repo"

g() { git -C "$repo" -c user.name=selftest -c user.email=selftest@example.invalid "$@"; }
g add -A
g commit -q -s --allow-empty -m "selftest: mirror working tree"

pass_count=0
fail_count=0

reset_repo() {
  g reset -q --hard HEAD
  g clean -qfdx
}

run_check() {
  (cd "$repo" && CHECK_DENYLIST="${DENY:-}" bash scripts/check.sh "$@") >"$out" 2>&1
}

ok() { printf 'ok     %s\n' "$1"; pass_count=$((pass_count + 1)); }
bad() {
  printf 'NOT OK %s: %s\n' "$1" "$2"
  sed 's/^/       | /' "$out"
  fail_count=$((fail_count + 1))
}

# expect_fail NAME PATTERN [check.sh args...]
expect_fail() {
  local name=$1 pattern=$2
  shift 2
  if run_check "$@"; then
    bad "$name" "check passed, expected a failure matching '$pattern'"
  elif ! grep -q -- "$pattern" "$out"; then
    bad "$name" "failed, but without '$pattern'"
  else
    ok "$name"
  fi
  reset_repo
}

# expect_pass NAME [check.sh args...]
expect_pass() {
  local name=$1
  shift
  if run_check "$@"; then
    ok "$name"
  else
    bad "$name" "check failed, expected a pass"
  fi
  reset_repo
}

put() { mkdir -p "$(dirname "$repo/$1")"; printf '%s\n' "$2" >"$repo/$1"; }
repeat() { local s=""; for ((i = 0; i < $2; i++)); do s+=$1; done; printf '%s' "$s"; }

# Violating strings, assembled so this file does not contain them.
aws_key="AKIA""IOSFODNN7EXAMPLE"
pem_header="-----BEGIN ""RSA PRIVATE KEY-----"
vendor_key="db-""$(repeat a 28)"
gh_token="ghp_""$(repeat b 36)"
hex_key_line="private_key = \"0x$(repeat ab 32)\""
exchange_id="GLBX"".MDP3"
deny_term="Acme Hidden"" Corp"

# Clean tree.
expect_pass "clean tree passes"

# 1. Data-file guard.
put notes/prices.csv "1,2"
expect_fail "data file outside fixtures" "data file outside fixtures"

put docs/PRICES.CSV "1,2"
expect_fail "uppercase data extension" "data file outside fixtures"

mkdir -p "$repo/archive" && printf 'x' >"$repo/archive/day.parquet" && g add -f archive/day.parquet
expect_fail "force-added ignored data file" "data file outside fixtures"

put docs/blob.zst "x"
expect_fail "compressed file outside fixtures" "data file outside fixtures"

put crates/x/src/blob.bin "x"
expect_fail "disallowed extension" "extension .bin is not allowed"

put scripts/blob "x"
expect_fail "unknown extensionless file" "extensionless file"

put crates/mesh-types/src/data.json "{}"
expect_fail "JSON outside allowed directories" "JSON is allowed only"

put docs/specs/example.json "{}"
expect_pass "JSON under docs/ is allowed"

head -c 1100000 /dev/zero | tr '\0' 'a' >"$repo/docs/big.md"
expect_fail "oversize file outside fixtures" "exceeds the 1 MiB cap"

put fixtures/other/a.csv "1"
expect_fail "fixture outside synthetic/admitted" "fixtures must live under"

head -c 5300000 /dev/zero | tr '\0' 'a' >"$repo/fixtures/synthetic/big.jsonl"
expect_fail "oversize fixture" "exceeds the 5 MiB fixture cap"

put fixtures/admitted/x/a.csv "1"
expect_fail "admitted fixture without ADMISSION" "without an ADMISSION file"

put fixtures/admitted/x/a.csv "1"
put fixtures/admitted/x/ADMISSION "covered by the trades admission"
expect_fail "ADMISSION without an ADM id" "does not name an admission record"

put fixtures/admitted/x/a.csv "1"
put docs/admissions/ADM-9998-selftest.md "- Status: Requested"
put fixtures/admitted/x/ADMISSION "ADM-9998"
expect_fail "ADMISSION naming a non-admitted record" "is not Admitted"

put fixtures/admitted/x/a.csv "1"
put fixtures/admitted/x/ADMISSION "ADM-9999"
expect_fail "ADMISSION naming an unknown record" "has no record"

put fixtures/admitted/x/a.csv "1"
put fixtures/admitted/x/ADMISSION "ADM-9998"
put docs/admissions/ADM-9998-selftest.md "$(printf '# ADM-9998: selftest\n\n- Status: Admitted\n')"
expect_pass "ADMISSION naming an admitted record"

# 2. Forbidden content.
put crates/x/src/lib.rs "const DATASET: &str = \"$exchange_id\";"
expect_fail "exchange identifier in code" "forbidden exchange-feed identifier"

put docs/excluded.md "We never ingest $exchange_id."
expect_pass "exchange identifier in documentation is allowed"

put docs/notes.md "key: $aws_key"
expect_fail "AWS key in markdown" "credential-like string"

put crates/x/src/key.rs "// $pem_header"
expect_fail "private key block in code" "credential-like string"

put docs/vendor.md "token $vendor_key"
expect_fail "vendor API key in markdown" "credential-like string"

put .github/workflows/x.yml "token: $gh_token"
expect_fail "GitHub token in workflow" "credential-like string"

put deploy.toml "$hex_key_line"
expect_fail "hex private key assignment" "credential-like string"

# 3. Denylist.
printf '%s\n' "$deny_term" >"$tmp/deny.txt"
put docs/leak.md "Built for $(printf '%s' "$deny_term" | tr '[:upper:]' '[:lower:]')."
DENY="$tmp/deny.txt" expect_fail "denylisted term (case-insensitive)" "denylisted term found"

DENY="$tmp/deny.txt" expect_pass "denylist with no matches"

DENY="$tmp/missing.txt" expect_fail "denylist path that does not exist" "is not a file"

printf '%s\n' "$deny_term" >"$repo/my-denylist.txt"
DENY="$repo/my-denylist.txt" expect_pass "untracked denylist inside the repo is skipped"

printf '%s\n' "$deny_term" >"$repo/my-denylist.txt"
g add my-denylist.txt
DENY="$repo/my-denylist.txt" expect_fail "tracked denylist inside the repo" "is tracked"

mkdir -p "$repo/crates/many"
for i in $(seq 1 3000); do printf '// %s\n' "$i" >"$repo/crates/many/f$i.rs"; done
printf '// %s\n' "$deny_term" >"$repo/crates/many/f3000.rs"
DENY="$tmp/deny.txt" expect_fail "denylisted term in the last of many files" "denylisted term found"

# 4. Markdown links.
put docs/linked.md "See [missing](no-such-file.md)."
expect_fail "broken relative link" "broken link no-such-file.md"

put docs/linked.md 'Example: `[text](not-a-link.md)` in inline code.'
expect_pass "link-like text in inline code is ignored"

# 5. DCO.
expect_pass "DCO over whole history (initial commit exempt)" --dco ROOT
expect_pass "DCO with an all-zero base (new branch push)" --dco 0000000000000000000000000000000000000000

put docs/unsigned.md "unsigned"
g add docs/unsigned.md
g commit -q -m "docs: unsigned change"
expect_fail "commit without sign-off" "has no Signed-off-by line" --dco HEAD~1
g reset -q --hard HEAD~1

printf '\n%d passed, %d failed\n' "$pass_count" "$fail_count"
if (( fail_count > 0 )); then
  exit 1
fi
