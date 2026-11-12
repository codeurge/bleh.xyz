variable "region" {
  description = "OCI home region (Always Free A1 capacity is only in the home region)."
  type        = string
  default     = "eu-frankfurt-1"
}

variable "compartment_id" {
  description = "Compartment OCID."
  type        = string
}

variable "availability_domain_index" {
  description = "Availability domain index; try another if A1 capacity is exhausted."
  type        = number
  default     = 0
}

variable "relays" {
  description = "Relay name to WireGuard overlay address (deploy/README.md, Overlay)."
  type = map(object({
    overlay_ip = string
  }))
  default = {
    relay-1 = { overlay_ip = "10.77.0.11" }
    relay-2 = { overlay_ip = "10.77.0.12" }
  }
}

variable "repo_ref" {
  description = "Commit SHA the hosts bootstrap from (a reviewed commit on main)."
  type        = string
}

variable "ssh_authorized_keys" {
  description = "Public SSH keys for the admin user."
  type        = list(string)
}

variable "ssh_cidrs" {
  description = "CIDRs allowed to reach SSH (empty: any; login is key-only and rate-limited)."
  type        = list(string)
  default     = []
}
