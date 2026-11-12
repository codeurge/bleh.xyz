variable "relays" {
  description = "Relay name (relay-1, ...) to its WireGuard overlay address."
  type = map(object({
    overlay_ip = string
  }))
}

variable "server_type" {
  description = "Hetzner Cloud server type. cax11 (arm64) or cx23 (x86); check availability and price."
  type        = string
  default     = "cax11"
}

variable "location" {
  description = "Hetzner Cloud location. EU locations include 20 TB of traffic; others much less."
  type        = string
  default     = "hel1"
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
