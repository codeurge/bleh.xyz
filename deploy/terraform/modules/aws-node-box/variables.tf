variable "site" {
  description = "Site label (a or b)."
  type        = string
}

variable "overlay_ip" {
  description = "WireGuard overlay address of this node box."
  type        = string
}

variable "instance_type" {
  description = "Instance type with 16 vCPU, 128 GiB and local NVMe. i4i.4xlarge has 3,750 GB of NVMe (about $1.61/h in Tokyo, November 2026; verify)."
  type        = string
  default     = "i4i.4xlarge"
}

variable "root_volume_gb" {
  description = "Root EBS volume size (OS, Docker images, raw staging)."
  type        = number
  default     = 100
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
  description = "Public SSH keys; the first also becomes the EC2 key pair."
  type        = list(string)
}

variable "ssh_cidrs" {
  description = "CIDRs allowed to reach SSH (empty: any)."
  type        = list(string)
  default     = []
}
