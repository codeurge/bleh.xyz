variable "account_id" {
  description = "Cloudflare account ID."
  type        = string
}

variable "zone_id" {
  description = "Cloudflare zone ID of the domain."
  type        = string
}

variable "domain" {
  description = "Apex domain."
  type        = string
  default     = "bleh.xyz"
}

variable "public_bucket" {
  description = "Public archive bucket."
  type        = string
  default     = "bleh-archive"
}

variable "private_bucket" {
  description = "Private unfiltered-raw bucket."
  type        = string
  default     = "bleh-raw-private"
}

variable "private_storage_class" {
  description = "Storage class of the private bucket (Standard or InfrequentAccess)."
  type        = string
  default     = "InfrequentAccess"
}

variable "r2_location" {
  description = "R2 location hint (weur, eeur, apac, ...), near the node box."
  type        = string
  default     = "weur"
}

variable "relays" {
  description = "Relay name to public IPv4, from the relays stack output relay_ips."
  type        = map(string)
  default     = {}
}

variable "node_boxes" {
  description = "Node box site labels; each gets a tunnel published as origin-<site>.<domain>."
  type        = list(string)
  default     = ["a"]
}

variable "home_tunnel" {
  description = "Create the home-archive tunnel (api-staging and grafana hostnames)."
  type        = bool
  default     = true
}

variable "operator_emails" {
  description = "People allowed through Access to Grafana and the staging API."
  type        = list(string)
  default     = []
}

variable "create_origin_cert" {
  description = "Create the Origin CA certificate the relays serve for live and api."
  type        = bool
  default     = true
}

variable "requests_per_10s" {
  description = "Per-IP request limit per 10 s on live and api."
  type        = number
  default     = 50
}
