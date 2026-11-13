#!/usr/bin/env bash
# Build bleh.xyz container images from a git commit, tagged by its SHA.
#
#   deploy/docker/build.sh [options] [target...]
#
# Targets: origin hl-node-reader gateway archive history-api swap-pollers
#          reference-pollers hl-backfill mesh-client tools hl-node
#          (default: all but hl-node)
#          origin is the node-box service; archive is built from
#          crates/origin; hl-node-reader is the standalone reader.
#
# Options:
#   --ref REF         commit to build (default HEAD)
#   --dirty           build the working tree (tracked + untracked, not
#                     ignored) instead of a commit; tagged <sha>-dirty
#   --registry PFX    image name prefix (default: $BLEH_REGISTRY or "bleh")
#   --push            push after building (needs `docker login`)
#   --platform P      target platform (default linux/amd64; the node binary
#                     is x86_64 only)
#   --also-tag TAG    extra tag for every image (for example "local")
#   --jobs N          parallel compile jobs (default: one per CPU of the
#                     builder); about 1.5 GiB of memory each
#
# The build context is a tar of the commit's files (`git archive`), so
# target/, .git and anything uncommitted never reach the daemon and the input
# is exactly the commit. SOURCE_DATE_EPOCH is the commit time, and BuildKit
# rewrites layer timestamps to it, so rebuilding a commit gives the same
# image config and layers inside the pinned build container.
set -euo pipefail

usage() { sed -n '2,27p' "$0" | sed 's/^# \{0,1\}//'; }

repo_root=$(git -C "$(dirname "$0")" rev-parse --show-toplevel)
cd "$repo_root"

ref=HEAD
dirty=0
registry=${BLEH_REGISTRY:-bleh}
push=0
platform=linux/amd64
extra_tags=()
jobs=
targets=()
all_targets=(origin hl-node-reader gateway archive history-api swap-pollers reference-pollers hl-backfill mesh-client tools)

while (($#)); do
  case "$1" in
    --ref) ref=${2:?}; shift 2 ;;
    --dirty) dirty=1; shift ;;
    --registry) registry=${2:?}; shift 2 ;;
    --push) push=1; shift ;;
    --platform) platform=${2:?}; shift 2 ;;
    --also-tag) extra_tags+=("${2:?}"); shift 2 ;;
    --jobs) jobs=${2:?}; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    -*) echo "unknown option $1" >&2; usage >&2; exit 64 ;;
    *) targets+=("$1"); shift ;;
  esac
done
((${#targets[@]})) || targets=("${all_targets[@]}")

if ! docker info >/dev/null 2>&1; then
  echo "build.sh: the Docker daemon is not reachable" >&2
  exit 69
fi
docker buildx version >/dev/null 2>&1 || { echo "build.sh: docker buildx is required" >&2; exit 69; }

sha=$(git rev-parse --verify "${ref}^{commit}")
short=${sha:0:12}
epoch=$(git log -1 --format=%ct "$sha")
tag=$short
context_tar=$(mktemp -t bleh-context.XXXXXX)
trap 'rm -f "$context_tar"' EXIT

if ((dirty)); then
  tag="${short}-dirty"
  epoch=$(date +%s)
  while IFS= read -r -d '' f; do
    if [[ -e "$f" ]]; then printf '%s\0' "$f"; fi
  done < <(git ls-files -z --cached --others --exclude-standard) \
    | tar --null -T - -cf "$context_tar"
else
  git archive --format=tar "$sha" > "$context_tar"
fi

push_flag=false
((push)) && push_flag=true

for t in "${targets[@]}"; do
  image="${registry}/${t}"
  tags=(-t "${image}:${tag}")
  for x in "${extra_tags[@]+"${extra_tags[@]}"}"; do tags+=(-t "${image}:${x}"); done

  if [[ "$t" == hl-node ]]; then
    dockerfile=deploy/docker/hl-node/Dockerfile
    target_args=()
  else
    dockerfile=deploy/docker/Dockerfile
    target_args=(--target "$t")
  fi

  echo "==> ${image}:${tag} (${dockerfile}${target_args[1]:+ target ${target_args[1]}})"
  if ((push)); then
    output="type=image,push=${push_flag},rewrite-timestamp=true"
  else
    output="type=docker,rewrite-timestamp=true"
  fi
  docker buildx build \
    --platform "$platform" \
    --file "$dockerfile" \
    "${target_args[@]+"${target_args[@]}"}" \
    --build-arg "SOURCE_DATE_EPOCH=${epoch}" \
    --build-arg "VCS_REF=${sha}" \
    --build-arg "BLEH_CARGO_JOBS=${jobs}" \
    --label "org.opencontainers.image.revision=${sha}" \
    --provenance=false \
    --output "$output" \
    "${tags[@]}" \
    - < "$context_tar"
done

echo "built: ${targets[*]} as ${registry}/<target>:${tag}"
