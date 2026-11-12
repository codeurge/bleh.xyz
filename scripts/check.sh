#!/usr/bin/env bash
# Repository checks. Run locally before opening a pull request; CI runs the same.
#
#   scripts/check.sh            run all checks on tracked files
#   scripts/check.sh --dco BASE also check DCO sign-off on commits BASE..HEAD
#
# Optional: point CHECK_DENYLIST at a local, untracked file of terms (one per
# line) that must never appear in the repository, such as private consumer
# names. The list itself is never committed.

set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

fail=0
err() { printf 'FAIL: %s\n' "$*" >&2; fail=1; }

tracked() { git ls-files -z --cached --others --exclude-standard; }

# 1. Data-file guard: market-data files may live only under fixtures/synthetic/
#    or fixtures/admitted/, and admitted fixtures need an ADMISSION file.
data_re='\.(dbn|zst|parquet|arrow|feather|csv|tsv|pcap|pcapng|h5|hdf5|ndjson|jsonl)$'
while IFS= read -r -d '' f; do
  if [[ "$f" =~ $data_re ]]; then
    case "$f" in
      fixtures/synthetic/*) ;;
      fixtures/admitted/*)
        dir=$(dirname "$f")
        [[ -f "$dir/ADMISSION" ]] || err "$f: admitted fixture without $dir/ADMISSION"
        ;;
      *) err "$f: data file outside fixtures/synthetic/ or fixtures/admitted/" ;;
    esac
  fi
done < <(tracked)

# 2. Forbidden content in non-documentation files: exchange-feed dataset and
#    venue identifiers, and credential-like strings. Documentation may name
#    them when explaining what is excluded.
forbidden='GLBX\.MDP3|OPRA\.PILLAR|XNAS\.ITCH|DBEQ\.|db-[A-Za-z0-9]{28}|AKIA[0-9A-Z]{16}|-----BEGIN [A-Z ]*PRIVATE KEY-----'
while IFS= read -r -d '' f; do
  case "$f" in
    *.md|scripts/check.sh|LICENSE|LICENSE-DATA) continue ;;
  esac
  [[ -f "$f" ]] || continue
  if grep -nIE "$forbidden" "$f" >/dev/null 2>&1; then
    err "$f: contains a forbidden exchange-feed identifier or credential pattern"
  fi
done < <(tracked)

# 3. Optional local denylist (never committed).
if [[ -n "${CHECK_DENYLIST:-}" && -f "$CHECK_DENYLIST" ]]; then
  while IFS= read -r term; do
    [[ -z "$term" || "$term" == \#* ]] && continue
    if tracked | xargs -0 grep -nIiF -- "$term" >/dev/null 2>&1; then
      err "denylisted term found in tracked files (term not printed)"
    fi
  done < "$CHECK_DENYLIST"
fi

# 4. Relative markdown links must resolve.
while IFS= read -r -d '' f; do
  [[ "$f" == *.md && -f "$f" ]] || continue
  dir=$(dirname "$f")
  # Ignore fenced code blocks, then extract ](target) links.
  awk '/^```/{c=!c; next} !c' "$f" \
    | grep -oE '\]\([^)]+\)' | sed -E 's/^\]\((.*)\)$/\1/' \
    | while IFS= read -r link; do
        link=${link%% *}
        case "$link" in
          http://*|https://*|mailto:*|\#*|'') continue ;;
        esac
        target=${link%%#*}
        [[ -e "$dir/$target" ]] || { printf 'FAIL: %s: broken link %s\n' "$f" "$link" >&2; echo broken; }
      done
done < <(tracked) | grep -q broken && fail=1

# 5. DCO sign-off on a commit range (CI passes the PR base).
if [[ "${1:-}" == "--dco" ]]; then
  base=${2:?usage: scripts/check.sh --dco BASE}
  while read -r sha; do
    git log -1 --format=%B "$sha" | grep -q '^Signed-off-by: ' \
      || err "commit $sha has no Signed-off-by line (use git commit -s)"
  done < <(git rev-list --no-merges "$base"..HEAD)
fi

if [[ $fail -ne 0 ]]; then
  echo "Checks failed." >&2
  exit 1
fi
echo "All checks passed."
