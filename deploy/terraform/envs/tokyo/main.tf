# Tier 3 (upgrade path, not the default): two node boxes in Tokyo on two
# independent providers, active-active as in ADR 0006. Relays, the edge and
# the home-archive server stay as in Tier 1; add both sites to
# envs/edge node_boxes and to the gateway upstreams.
#
# Credentials: AWS_PROFILE or AWS_* variables; VULTR_API_KEY.

provider "aws" {
  region = "ap-northeast-1"

  default_tags {
    tags = { project = "bleh" }
  }
}

provider "vultr" {}

module "site_a" {
  source = "../../modules/aws-node-box"
  count  = var.enable_aws ? 1 : 0

  site                = "a"
  overlay_ip          = "10.77.0.1"
  instance_type       = var.aws_instance_type
  repo_ref            = var.repo_ref
  ssh_authorized_keys = var.ssh_authorized_keys
  ssh_cidrs           = var.ssh_cidrs
}

module "site_b" {
  source = "../../modules/vultr-node-box"
  count  = var.enable_vultr ? 1 : 0

  site                = "b"
  overlay_ip          = "10.77.0.3"
  plan                = var.vultr_plan
  repo_ref            = var.repo_ref
  ssh_authorized_keys = var.ssh_authorized_keys
  ssh_cidrs           = var.ssh_cidrs
}
