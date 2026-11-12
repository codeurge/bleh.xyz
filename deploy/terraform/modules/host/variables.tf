variable "role" {
  description = "Host role for deploy/scripts/host-bootstrap.sh: node-box, home-archive, relay or origin."
  type        = string

  validation {
    condition     = contains(["node-box", "home-archive", "relay", "origin"], var.role)
    error_message = "The role must be node-box, home-archive, relay or origin."
  }
}

variable "site" {
  description = "Site label for metrics and logs, for example a, b or relay-1."
  type        = string
}

variable "overlay_ip" {
  description = "WireGuard overlay address (deploy/README.md, Overlay)."
  type        = string
}

variable "repo_url" {
  description = "Repository to clone on the host."
  type        = string
  default     = "https://github.com/codeurge/bleh.xyz.git"
}

variable "repo_ref" {
  description = "Commit SHA to check out. Pin a reviewed commit; never a branch name."
  type        = string

  validation {
    condition     = can(regex("^[0-9a-f]{40}$", var.repo_ref))
    error_message = "repo_ref must be a full 40-character commit SHA."
  }
}

variable "admin_user" {
  description = "Login user created by cloud-init (sudo, key-only)."
  type        = string
  default     = "ops"
}

variable "ssh_authorized_keys" {
  description = "Public SSH keys for the admin user."
  type        = list(string)
}

variable "ssh_cidrs" {
  description = "CIDRs allowed to reach SSH. Empty allows any address (key-only login, rate-limited)."
  type        = list(string)
  default     = []
}

variable "auto_reboot" {
  description = "Reboot automatically for security updates at 04:30 UTC. True for relays, false for node boxes."
  type        = bool
  default     = false
}
