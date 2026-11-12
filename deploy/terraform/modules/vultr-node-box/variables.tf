variable "site" {
  description = "Site label (a or b)."
  type        = string
}

variable "overlay_ip" {
  description = "WireGuard overlay address of this node box."
  type        = string
}

variable "region" {
  description = "Vultr region; nrt is Tokyo."
  type        = string
  default     = "nrt"
}

variable "plan" {
  description = "Bare-metal plan. vbm-8c-132gb (8 cores / 16 threads, 128 GB, 1.92 TB NVMe, $350/month, November 2026) meets the stated minimum exactly; verify stock in the region."
  type        = string
  default     = "vbm-8c-132gb"
}

variable "repo_url" {
  description = "Repository cloned on the host."
  type        = string
  default     = "https://github.com/codeurge/bleh.xyz.git"
}

variable "repo_ref" {
  description = "Commit SHA the host bootstraps from."
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
