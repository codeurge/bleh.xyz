variable "account_id" {
  description = "Cloudflare account ID."
  type        = string
}

variable "zone_id" {
  description = "Cloudflare zone ID for the tunnel hostnames."
  type        = string
}

variable "name" {
  description = "Tunnel name, for example bleh-node-a or bleh-home."
  type        = string
}

variable "ingress" {
  description = "Published hostnames: service URL as seen from the cloudflared container, and who may connect (service_token or email)."
  type = list(object({
    hostname = string
    service  = string
    access   = string
    emails   = optional(list(string), [])
  }))

  validation {
    condition     = alltrue([for i in var.ingress : contains(["service_token", "email"], i.access)])
    error_message = "Each ingress access must be service_token or email."
  }

  validation {
    condition     = alltrue([for i in var.ingress : i.access != "email" || length(i.emails) > 0])
    error_message = "An email-gated hostname needs at least one email."
  }
}

variable "service_token_duration" {
  description = "Lifetime of the Access service token; rotate before it ends (docs/operations/keys.md)."
  type        = string
  default     = "8760h"
}

variable "service_token_id" {
  description = "Existing Access service token to admit on service_token hostnames (shared by several tunnels). Null creates one for this tunnel."
  type        = string
  default     = null
}
