variable "zone_id" {
  description = "Route 53 hosted zone ID of the fallback domain."
  type        = string
}

variable "domain" {
  description = "Fallback domain, registered with a different registrar than the primary."
  type        = string
}

variable "relays" {
  description = "Relay name to public IPv4 (same map as envs/edge)."
  type        = map(string)
}
