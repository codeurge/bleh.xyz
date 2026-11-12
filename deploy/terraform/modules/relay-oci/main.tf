# Relays on Oracle Cloud Always Free (Ampere A1, arm64). The free
# allowance is 2 OCPU and 12 GB in total (halved in 2026; verify current
# terms) and 10 TB of outbound data a month, so the default is two relays
# of 1 OCPU / 6 GB each. Oracle may reclaim idle free instances; relays are
# stateless and re-created with one apply. Images must be multi-arch
# (deploy/ci/deploy.yml builds linux/arm64).

terraform {
  required_version = ">= 1.10"
  required_providers {
    oci = {
      source  = "oracle/oci"
      version = "~> 7.0"
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
  # Every relay has the same rules; take them from any instance of the host module.
  rules = length(var.relays) > 0 ? values(module.host)[0].firewall_rules : []
}

data "oci_identity_availability_domains" "ads" {
  compartment_id = var.compartment_id
}

data "oci_core_images" "ubuntu" {
  compartment_id           = var.compartment_id
  operating_system         = "Canonical Ubuntu"
  operating_system_version = "24.04"
  shape                    = "VM.Standard.A1.Flex"
  sort_by                  = "TIMECREATED"
  sort_order               = "DESC"
}

resource "oci_core_vcn" "this" {
  compartment_id = var.compartment_id
  cidr_blocks    = ["10.60.0.0/16"]
  display_name   = "bleh-relays"
  dns_label      = "blehrelays"
}

resource "oci_core_internet_gateway" "this" {
  compartment_id = var.compartment_id
  vcn_id         = oci_core_vcn.this.id
  display_name   = "bleh-relays"
  enabled        = true
}

resource "oci_core_route_table" "this" {
  compartment_id = var.compartment_id
  vcn_id         = oci_core_vcn.this.id
  display_name   = "bleh-relays"

  route_rules {
    destination       = "0.0.0.0/0"
    destination_type  = "CIDR_BLOCK"
    network_entity_id = oci_core_internet_gateway.this.id
  }
}

resource "oci_core_security_list" "this" {
  compartment_id = var.compartment_id
  vcn_id         = oci_core_vcn.this.id
  display_name   = "bleh-relays"

  egress_security_rules {
    destination = "0.0.0.0/0"
    protocol    = "all"
  }

  dynamic "ingress_security_rules" {
    for_each = flatten([for r in local.rules : [for c in r.cidrs : merge(r, { cidr = c })]])
    content {
      description = ingress_security_rules.value.description
      source      = ingress_security_rules.value.cidr
      protocol    = ingress_security_rules.value.protocol == "tcp" ? "6" : "17"

      dynamic "tcp_options" {
        for_each = ingress_security_rules.value.protocol == "tcp" ? [1] : []
        content {
          min = ingress_security_rules.value.from_port
          max = ingress_security_rules.value.to_port
        }
      }

      dynamic "udp_options" {
        for_each = ingress_security_rules.value.protocol == "udp" ? [1] : []
        content {
          min = ingress_security_rules.value.from_port
          max = ingress_security_rules.value.to_port
        }
      }
    }
  }

  ingress_security_rules {
    description = "ICMP path MTU discovery"
    source      = "0.0.0.0/0"
    protocol    = "1"
    icmp_options {
      type = 3
      code = 4
    }
  }
}

resource "oci_core_subnet" "this" {
  compartment_id    = var.compartment_id
  vcn_id            = oci_core_vcn.this.id
  cidr_block        = "10.60.1.0/24"
  display_name      = "bleh-relays"
  dns_label         = "relays"
  route_table_id    = oci_core_route_table.this.id
  security_list_ids = [oci_core_security_list.this.id]
}

resource "oci_core_instance" "relay" {
  for_each = var.relays

  compartment_id      = var.compartment_id
  availability_domain = data.oci_identity_availability_domains.ads.availability_domains[var.availability_domain_index].name
  display_name        = "bleh-${each.key}"
  shape               = "VM.Standard.A1.Flex"

  shape_config {
    ocpus         = var.ocpus_per_relay
    memory_in_gbs = var.memory_gb_per_relay
  }

  source_details {
    source_type             = "image"
    source_id               = data.oci_core_images.ubuntu.images[0].id
    boot_volume_size_in_gbs = 50
  }

  create_vnic_details {
    subnet_id        = oci_core_subnet.this.id
    assign_public_ip = true
    hostname_label   = each.key
  }

  metadata = {
    ssh_authorized_keys = join("\n", var.ssh_authorized_keys)
    user_data           = base64encode(module.host[each.key].user_data)
  }

  lifecycle {
    # A newer image or a changed bootstrap must not replace a running relay
    # silently; re-create relays deliberately (docs/operations/deploy.md).
    ignore_changes = [source_details[0].source_id, metadata]
  }
}
