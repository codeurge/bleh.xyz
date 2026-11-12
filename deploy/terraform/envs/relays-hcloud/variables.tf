variable "server_type" {
  description = "Server type (cax11 arm64, cx23 x86)."
  type        = string
  default     = "cax11"
}

variable "location" {
  description = "Location (EU locations include 20 TB of traffic)."
  type        = string
  default     = "hel1"
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
