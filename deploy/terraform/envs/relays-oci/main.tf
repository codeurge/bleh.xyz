# Relays on Oracle Cloud Always Free (default for ADR 0010). Credentials:
# ~/.oci/config or the OCI_* environment variables
# (docs/operations/keys.md, "Provider credentials").

provider "oci" {
  region = var.region
}

module "relays" {
  source = "../../modules/relay-oci"

  compartment_id            = var.compartment_id
  availability_domain_index = var.availability_domain_index
  relays                    = var.relays
  repo_ref                  = var.repo_ref
  ssh_authorized_keys       = var.ssh_authorized_keys
  ssh_cidrs                 = var.ssh_cidrs
}
