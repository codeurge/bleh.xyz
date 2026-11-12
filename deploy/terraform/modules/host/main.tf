# Provider-agnostic host definition. Renders the cloud-init user data that
# clones the repository at a pinned commit and runs
# deploy/scripts/host-bootstrap.sh for the role, and describes the inbound
# rules the role needs so each provider module can build its own firewall
# from one list. The host also enforces the same policy with nftables.

terraform {
  required_version = ">= 1.10"
}

locals {
  ssh_cidrs = length(var.ssh_cidrs) > 0 ? var.ssh_cidrs : ["0.0.0.0/0"]

  bootstrap_args = join(" ", concat(
    [var.role, "--site", var.site, "--overlay-ip", var.overlay_ip],
    flatten([for c in var.ssh_cidrs : ["--ssh-cidr", c]]),
    var.auto_reboot ? ["--auto-reboot"] : [],
  ))

  user_data = templatefile("${path.module}/templates/cloud-init.yaml", {
    admin_user          = var.admin_user
    ssh_authorized_keys = jsonencode(var.ssh_authorized_keys)
    repo_url            = var.repo_url
    repo_ref            = var.repo_ref
    bootstrap_args      = local.bootstrap_args
  })

  common_rules = [
    { description = "SSH", protocol = "tcp", from_port = 22, to_port = 22, cidrs = local.ssh_cidrs },
    { description = "WireGuard overlay", protocol = "udp", from_port = 51820, to_port = 51820, cidrs = ["0.0.0.0/0"] },
  ]

  role_rules = {
    node-box = [
      { description = "Hyperliquid gossip", protocol = "tcp", from_port = 4001, to_port = 4002, cidrs = ["0.0.0.0/0"] },
    ]
    relay = [
      { description = "HTTP (ACME, redirect)", protocol = "tcp", from_port = 80, to_port = 80, cidrs = ["0.0.0.0/0"] },
      { description = "HTTPS", protocol = "tcp", from_port = 443, to_port = 443, cidrs = ["0.0.0.0/0"] },
      { description = "HTTP/3", protocol = "udp", from_port = 443, to_port = 443, cidrs = ["0.0.0.0/0"] },
    ]
    home-archive = []
    origin = [
      { description = "Hyperliquid gossip", protocol = "tcp", from_port = 4001, to_port = 4002, cidrs = ["0.0.0.0/0"] },
    ]
  }
}
