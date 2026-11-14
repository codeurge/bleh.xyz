# Bake definition for the CI image build (.github/workflows/deploy.yml).
#
# Every service target shares the `build` stage of deploy/docker/Dockerfile
# with identical arguments, so building them in one `docker buildx bake`
# invocation compiles the workspace once per platform, not once per image.
#
# Each image is pushed by digest only (no tag). The workflow's publish job
# merges the per-platform digests into one tag per image, `<sha>`, and signs
# it. Local builds keep using deploy/docker/build.sh.
#
#   REGISTRY=ghcr.io/codeurge/bleh.xyz VCS_REF=<sha> SOURCE_DATE_EPOCH=<t> \
#     docker buildx bake -f deploy/docker/docker-bake.hcl amd64 \
#     --set '*.platform=linux/amd64'

variable "REGISTRY" {
  default = "bleh"
}

variable "VCS_REF" {
  default = "unknown"
}

variable "SOURCE_DATE_EPOCH" {
  default = "0"
}

# Every image, built for linux/amd64 (node boxes).
group "amd64" {
  targets = ["service", "hl-node"]
}

# Images the default relays also run, built for linux/arm64.
group "arm64" {
  targets = ["gateway", "history-api", "tools"]
}

target "_common" {
  context = "."
  args = {
    SOURCE_DATE_EPOCH = SOURCE_DATE_EPOCH
    VCS_REF           = VCS_REF
  }
  labels = {
    "org.opencontainers.image.revision" = VCS_REF
  }
  attest = [
    "type=provenance,mode=max",
    "type=sbom",
  ]
}

target "service" {
  name       = tgt
  inherits   = ["_common"]
  dockerfile = "deploy/docker/Dockerfile"
  target     = tgt
  matrix = {
    tgt = ["origin", "hl-node-reader", "gateway", "archive", "history-api",
    "swap-pollers", "reference-pollers", "hl-backfill", "tools"]
  }
  output = ["type=image,name=${REGISTRY}/${tgt},push-by-digest=true,name-canonical=true,push=true,rewrite-timestamp=true"]
}

target "hl-node" {
  inherits   = ["_common"]
  dockerfile = "deploy/docker/hl-node/Dockerfile"
  output     = ["type=image,name=${REGISTRY}/hl-node,push-by-digest=true,name-canonical=true,push=true,rewrite-timestamp=true"]
}
