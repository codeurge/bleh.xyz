variable "compartment_id" {
  description = "OCI compartment OCID for the relays."
  type        = string
}

variable "availability_domain_index" {
  description = "Index of the availability domain to use (A1 capacity varies between domains)."
  type        = number
  default     = 0
}

variable "relays" {
  description = "Relay name (relay-1, ...) to its WireGuard overlay address."
  type = map(object({
    overlay_ip = string
  }))
}

variable "ocpus_per_relay" {
  description = "OCPUs per relay. The Always Free total is 2 (verify current terms)."
  type        = number
  default     = 1
}

variable "memory_gb_per_relay" {
  description = "Memory per relay in GB. The Always Free total is 12 (verify current terms)."
  type        = number
  default     = 6
}

variable "repo_url" {
  description = "Repository cloned on the relays."
  type        = string
  default     = "https://github.com/codeurge/bleh.xyz.git"
}

variable "repo_ref" {
  description = "Commit SHA the relays bootstrap from."
  type        = string
}

variable "ssh_authorized_keys" {
  description = "Public SSH keys for the admin user."
  type        = list(string)
}

variable "ssh_cidrs" {
  description = "CIDRs allowed to reach SSH (empty: any)."
  type        = list(string)
  default     = []
}
