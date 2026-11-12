#!/usr/bin/env bash
# Repository checks. Run locally before opening a pull request; CI runs the same.
#
#   scripts/check.sh              run all checks on tracked and untracked
#                                 (not ignored) files
#   scripts/check.sh --dco BASE   also check DCO sign-off on commits BASE..HEAD;
#                                 BASE may be ROOT for the whole history
#
# Optional: point CHECK_DENYLIST at a local file of terms (one per line) that
# must never appear in the repository, such as private consumer names. The
# list itself is never committed; if it lives inside the repository it is
# skipped by the scan and must be untracked.
#
# scripts/check-selftest.sh proves that each check catches what it should.

set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(pwd -P)

fail=0
err() { printf 'FAIL: %s\n' "$*" >&2; fail=1; }

# Commits exempt from the DCO check: the initial design commit predates the
# sign-off rule.
dco_exempt=(
  367bcaf055d30ca5e9cb0ad6154b5f6f4b152d9c
)

# Size caps in bytes.
max_size_outside_fixtures=1048576   # 1 MiB
max_size_in_fixtures=5242880        # 5 MiB
max_size_brand=2097152              # 2 MiB
max_size_brand_video=20971520       # 20 MiB
brand_ext=' svg html css js mjs json webmanifest png ico webp gif jpg jpeg mp4 webm woff2 txt md '
brand_video_ext=' mp4 webm gif '

# Every file the checks consider: tracked (including force-added ignored
# files) plus untracked files that are not ignored. NUL-separated.
files() { git ls-files -z --cached --others --exclude-standard | sort -zu; }

# Resolve the denylist path, if any, and whether it is inside the repository.
denylist=""
denylist_rel=""
if [[ -n "${CHECK_DENYLIST:-}" ]]; then
  if [[ -f "$CHECK_DENYLIST" ]]; then
    denylist=$(cd "$(dirname "$CHECK_DENYLIST")" && pwd -P)/$(basename "$CHECK_DENYLIST")
    case "$denylist" in
      "$root"/*) denylist_rel=${denylist#"$root"/} ;;
    esac
  else
    err "CHECK_DENYLIST is set but $CHECK_DENYLIST is not a file"
  fi
fi

# in_list LIST WORD: true if WORD is in the space-delimited LIST, ignoring case.
in_list() {
  local found=1
  shopt -s nocasematch
  [[ "$1" == *" $2 "* ]] && found=0
  shopt -u nocasematch
  return $found
}

# 1. Data-file guard.
#    - Market-data files (by extension, case-insensitive) may live only under
#      fixtures/synthetic/ or fixtures/admitted/.
#    - Admitted fixtures need an ADMISSION file naming an Admitted ADM record.
#    - Outside fixtures/, only allowlisted extensions and file names are
#      permitted, JSON only in documentation, CI and test directories, and
#      every file is capped at 1 MiB. Files in fixtures/ are capped at 5 MiB.
data_ext=' dbn zst parquet arrow feather ipc csv tsv pcap pcapng h5 hdf5 ndjson jsonl gz bz2 xz zip tar 7z rmp msgpack avro orc '
allowed_ext=' rs toml md yml yaml sh lock tf hcl py pyi ts tsx js mjs cjs css html service timer conf ini sql txt '
allowed_names=' LICENSE LICENSE-DATA CODEOWNERS Dockerfile .gitignore .gitattributes .editorconfig .dockerignore ADMISSION '

check_admission() {
  local dir=$1 adm
  if [[ ! -f "$dir/ADMISSION" ]]; then
    err "$dir: admitted fixture directory without an ADMISSION file"
    return
  fi
  adm=$(grep -oE 'ADM-[0-9]{4,}' "$dir/ADMISSION" | head -n 1 || true)
  if [[ -z "$adm" ]]; then
    err "$dir/ADMISSION: does not name an admission record (ADM-NNNN)"
    return
  fi
  local record
  record=$(find docs/admissions -maxdepth 1 -name "$adm-*.md" -print -quit 2>/dev/null || true)
  if [[ -z "$record" ]]; then
    err "$dir/ADMISSION: $adm has no record in docs/admissions/"
  elif ! grep -qE '^- Status: Admitted' "$record"; then
    err "$dir/ADMISSION: $adm is not Admitted"
  fi
}

declare -a admitted_dirs=()
while IFS= read -r -d '' f; do
  [[ -f "$f" ]] || continue
  base=${f##*/}
  ext=""
  [[ "$base" == *.* ]] && ext=${base##*.}
  size=$(wc -c < "$f" | tr -d ' ')

  case "$f" in
    fixtures/synthetic/*)
      (( size <= max_size_in_fixtures )) || err "$f: $size bytes exceeds the 5 MiB fixture cap"
      continue
      ;;
    fixtures/admitted/*)
      (( size <= max_size_in_fixtures )) || err "$f: $size bytes exceeds the 5 MiB fixture cap"
      admitted_dirs+=("$(dirname "$f")")
      continue
      ;;
    fixtures/*)
      err "$f: fixtures must live under fixtures/synthetic/ or fixtures/admitted/"
      continue
      ;;
    brand/*)
      # Brand assets: pages, images, fonts and rendered video. Never data.
      if [[ -n "$ext" ]] && in_list "$brand_ext" "$ext"; then
        cap=$max_size_brand
        in_list "$brand_video_ext" "$ext" && cap=$max_size_brand_video
        (( size <= cap )) || err "$f: $size bytes exceeds the brand cap of $cap bytes"
        continue
      fi
      ;;
  esac

  if [[ -n "$ext" ]] && in_list "$data_ext" "$ext"; then
    err "$f: data file outside fixtures/synthetic/ or fixtures/admitted/"
    continue
  fi
  (( size <= max_size_outside_fixtures )) || err "$f: $size bytes exceeds the 1 MiB cap outside fixtures/"

  [[ -n "$denylist_rel" && "$f" == "$denylist_rel" ]] && continue
  if [[ "$allowed_names" == *" $base "* ]]; then
    continue
  fi
  if in_list " json " "$ext"; then
    case "$f" in
      docs/*|.github/*|crates/*/tests/*|tools/*/tests/*|deploy/*|sdks/*) ;;
      *) err "$f: JSON is allowed only under docs/, .github/ and crate tests/ directories" ;;
    esac
    continue
  fi
  if [[ -z "$ext" ]]; then
    err "$f: extensionless file is not on the allowlist of known names"
    continue
  fi
  in_list "$allowed_ext" "$ext" || err "$f: extension .$ext is not allowed outside fixtures/"
done < <(files)

if (( ${#admitted_dirs[@]} > 0 )); then
  while IFS= read -r dir; do
    check_admission "$dir"
  done < <(printf '%s\n' "${admitted_dirs[@]}" | sort -u)
fi

# git grep over the same file set: tracked and untracked-not-ignored files,
# in one process with no argument batching. Sets GREP_OUT to the matching
# paths (one per line). Runs in the current shell so a git grep error (exit
# status above 1) fails the checks instead of reading as "no match".
GREP_OUT=""
grep_files() {
  # usage: grep_files <grep options...> -e PATTERN [-- pathspec...]
  local status=0
  GREP_OUT=$(git grep --untracked -I -l "$@") || status=$?
  if (( status > 1 )); then
    err "git grep failed with status $status"
    GREP_OUT=""
  fi
}

denylist_exclude=()
[[ -n "$denylist_rel" ]] && denylist_exclude=(":(exclude,literal)$denylist_rel")

# 2a. Exchange-feed dataset and venue identifiers in non-documentation files.
#     Documentation may name them when explaining what is excluded.
exchange_ids='GLBX\.MDP3|OPRA\.PILLAR|XNAS\.ITCH|DBEQ\.'
grep_files -E -e "$exchange_ids" -- . ':(exclude)*.md' ':(exclude)scripts/check.sh' \
  ':(exclude)LICENSE' ':(exclude)LICENSE-DATA' ${denylist_exclude[@]+"${denylist_exclude[@]}"}
while IFS= read -r f; do
  [[ -n "$f" ]] && err "$f: contains a forbidden exchange-feed identifier"
done <<< "$GREP_OUT"

# 2b. Credential-like strings in every file, documentation included.
credentials=(
  '(AKIA|ASIA)[0-9A-Z]{16}'                                   # AWS access key ID
  'aws_secret_access_key[[:space:]]*[:=][[:space:]]*[A-Za-z0-9/+]{40}'
  '-----BEGIN [A-Z ]*PRIVATE KEY-----'                        # PEM private keys
  'db-[A-Za-z0-9]{28}'                                        # market-data vendor API key
  'gh[pousr]_[A-Za-z0-9]{36}'                                 # GitHub tokens
  'github_pat_[A-Za-z0-9_]{60,}'
  'xox[abprs]-[A-Za-z0-9-]{10,}'                              # Slack tokens
  '(sk|rk)_live_[A-Za-z0-9]{20,}'                             # Stripe live keys
  'AIza[0-9A-Za-z_-]{35}'                                     # Google API keys
  'sk-(ant|proj)-[A-Za-z0-9_-]{20,}'                          # LLM provider keys
  '([Pp]rivate|PRIVATE|[Ss]ecret|SECRET)[_-]?(key|KEY)[[:space:]]*[:=][[:space:]]*["'"'"']?(0x)?[0-9a-fA-F]{64}'
)
cred_args=()
for p in "${credentials[@]}"; do
  cred_args+=(-e "$p")
done
grep_files -E "${cred_args[@]}" -- . ${denylist_exclude[@]+"${denylist_exclude[@]}"}
while IFS= read -r f; do
  [[ -n "$f" ]] && err "$f: contains a credential-like string"
done <<< "$GREP_OUT"

# 3. Optional local denylist (never committed).
if [[ -n "$denylist" ]]; then
  if [[ -n "$denylist_rel" ]] && git ls-files --error-unmatch -- "$denylist_rel" >/dev/null 2>&1; then
    err "the denylist file $denylist_rel is tracked; it must never be committed"
  fi
  while IFS= read -r term || [[ -n "$term" ]]; do
    term=${term%$'\r'}
    [[ -z "$term" || "$term" == \#* ]] && continue
    grep_files -i -F -e "$term" -- . ${denylist_exclude[@]+"${denylist_exclude[@]}"}
    if [[ -n "$GREP_OUT" ]]; then
      err "denylisted term found in repository files (term not printed)"
    fi
  done < "$denylist"
fi

# 4. Relative markdown links must resolve.
broken=0
while IFS= read -r -d '' f; do
  [[ "$f" == *.md && -f "$f" ]] || continue
  dir=$(dirname "$f")
  # Ignore fenced code blocks and inline code, then extract ](target) links.
  while IFS= read -r link; do
    link=${link%% *}
    case "$link" in
      http://*|https://*|mailto:*|\#*|'') continue ;;
    esac
    target=${link%%#*}
    if [[ ! -e "$dir/$target" ]]; then
      printf 'FAIL: %s: broken link %s\n' "$f" "$link" >&2
      broken=1
    fi
  done < <(awk '/^[[:space:]]*```/{c=!c; next} !c' "$f" | sed -E 's/`[^`]*`//g' \
             | grep -oE '\]\([^)]+\)' | sed -E 's/^\]\((.*)\)$/\1/' || true)
done < <(files)
(( broken == 0 )) || fail=1

# 5. DCO sign-off on a commit range (CI passes the PR base or the push's
#    previous head; ROOT checks the whole history).
if [[ "${1:-}" == "--dco" ]]; then
  base=${2:?usage: scripts/check.sh --dco BASE|ROOT}
  if [[ "$base" == ROOT || "$base" =~ ^0+$ ]]; then
    range=(HEAD)
  else
    range=("$base..HEAD")
  fi
  while read -r sha; do
    [[ -z "$sha" ]] && continue
    exempt=0
    for e in "${dco_exempt[@]}"; do
      [[ "$sha" == "$e" ]] && exempt=1
    done
    (( exempt )) && continue
    git log -1 --format=%B "$sha" | grep -q '^Signed-off-by: ' \
      || err "commit $sha has no Signed-off-by line (use git commit -s)"
  done < <(git rev-list --no-merges "${range[@]}")
fi

if [[ $fail -ne 0 ]]; then
  echo "Checks failed." >&2
  exit 1
fi
echo "All checks passed."
