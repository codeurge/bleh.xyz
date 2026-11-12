# Tier 3 node box on Vultr bare metal in Tokyo (nrt), the second provider
# for active-active. Vultr firewall groups do not apply to bare metal, so the
# host's nftables policy (host-bootstrap.sh) is the only filter. Check plan
# availability in nrt first: `vultr-cli plans metal list`.

terraform {
  required_version = ">= 1.10"
  required_providers {
    vultr = {
      source  = "vultr/vultr"
      version = "~> 2.27"
    }
  }
}

module "host" {
  source = "../host"

  role                = "node-box"
  site                = var.site
  overlay_ip          = var.overlay_ip
  repo_url            = var.repo_url
  repo_ref            = var.repo_ref
  ssh_authorized_keys = var.ssh_authorized_keys
  ssh_cidrs           = var.ssh_cidrs
}

data "vultr_os" "ubuntu" {
  filter {
    name   = "name"
    values = ["Ubuntu 24.04 LTS x64"]
  }
}

resource "vultr_ssh_key" "this" {
  for_each = { for i, k in var.ssh_authorized_keys : "bleh-${var.site}-${i}" => k }

  name    = each.key
  ssh_key = each.value
}

resource "vultr_bare_metal_server" "this" {
  region      = var.region
  plan        = var.plan
  os_id       = data.vultr_os.ubuntu.id
  label       = "bleh-${var.site}-node-box"
  hostname    = "bleh-${var.site}"
  enable_ipv6 = true
  ssh_key_ids = [for k in vultr_ssh_key.this : k.id]
  user_data   = module.host.user_data
  tags        = ["bleh", "node-box"]

  lifecycle {
    ignore_changes = [user_data, os_id]
  }
}
