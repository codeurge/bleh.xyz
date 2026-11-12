variable "enable_aws" {
  description = "Create node box a on AWS ap-northeast-1."
  type        = bool
  default     = true
}

variable "enable_vultr" {
  description = "Create node box b on Vultr bare metal in Tokyo."
  type        = bool
  default     = true
}

variable "aws_instance_type" {
  description = "AWS instance type for node box a."
  type        = string
  default     = "i4i.4xlarge"
}

variable "vultr_plan" {
  description = "Vultr bare-metal plan for node box b."
  type        = string
  default     = "vbm-8c-132gb"
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
