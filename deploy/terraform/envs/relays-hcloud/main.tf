# Relays on Hetzner Cloud (paid fallback for the Oracle free tier).
# Credentials: HCLOUD_TOKEN in the environment.

provider "hcloud" {}

module "relays" {
  source = "../../modules/relay-hcloud"

  relays              = var.relays
  server_type         = var.server_type
  location            = var.location
  repo_ref            = var.repo_ref
  ssh_authorized_keys = var.ssh_authorized_keys
  ssh_cidrs           = var.ssh_cidrs
}
