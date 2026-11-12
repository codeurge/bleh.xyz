# Relays on Hetzner Cloud: a few euros a month each with 20 TB of traffic
# included in EU locations (verify current plans and prices). The
# fallback when Oracle's free tier is unavailable or reclaimed.

terraform {
  required_version = ">= 1.10"
  required_providers {
    hcloud = {
      source  = "hetznercloud/hcloud"
      version = "~> 1.52"
    }
  }
}

module "host" {
  source   = "../host"
  for_each = var.relays

  role                = "relay"
  site                = each.key
  overlay_ip          = each.value.overlay_ip
  repo_url            = var.repo_url
  repo_ref            = var.repo_ref
  ssh_authorized_keys = var.ssh_authorized_keys
  ssh_cidrs           = var.ssh_cidrs
  auto_reboot         = true
}

locals {
  rules = length(var.relays) > 0 ? values(module.host)[0].firewall_rules : []
}

resource "hcloud_ssh_key" "this" {
  for_each = { for i, k in var.ssh_authorized_keys : "bleh-${i}" => k }

  name       = each.key
  public_key = each.value
}

resource "hcloud_firewall" "relay" {
  name = "bleh-relays"

  dynamic "rule" {
    for_each = local.rules
    content {
      description = rule.value.description
      direction   = "in"
      protocol    = rule.value.protocol
      port        = rule.value.from_port == rule.value.to_port ? tostring(rule.value.from_port) : "${rule.value.from_port}-${rule.value.to_port}"
      source_ips  = concat(rule.value.cidrs, contains(rule.value.cidrs, "0.0.0.0/0") ? ["::/0"] : [])
    }
  }

  rule {
    description = "ICMP"
    direction   = "in"
    protocol    = "icmp"
    source_ips  = ["0.0.0.0/0", "::/0"]
  }
}

resource "hcloud_server" "relay" {
  for_each = var.relays

  name         = "bleh-${each.key}"
  server_type  = var.server_type
  image        = "ubuntu-24.04"
  location     = var.location
  ssh_keys     = [for k in hcloud_ssh_key.this : k.id]
  firewall_ids = [hcloud_firewall.relay.id]
  user_data    = module.host[each.key].user_data
  labels = {
    project = "bleh"
    role    = "relay"
  }

  public_net {
    ipv4_enabled = true
    ipv6_enabled = true
  }

  lifecycle {
    ignore_changes = [user_data, image]
  }
}
