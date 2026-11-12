variable "account_id" {
  description = "Cloudflare account ID."
  type        = string
}

variable "zone_id" {
  description = "Cloudflare zone ID of the domain."
  type        = string
}

variable "domain" {
  description = "Apex domain, for example bleh.xyz."
  type        = string
}

variable "public_bucket" {
  description = "R2 bucket for the public archive (filtered raw, partitions, manifests)."
  type        = string
  default     = "bleh-archive"
}

variable "private_bucket" {
  description = "R2 bucket for unfiltered raw (private)."
  type        = string
  default     = "bleh-raw-private"
}

variable "private_storage_class" {
  description = "Storage class for the private bucket: Standard or InfrequentAccess (cheaper storage, paid retrieval, 30-day minimum)."
  type        = string
  default     = "InfrequentAccess"
}

variable "r2_location" {
  description = "R2 location hint: weur or eeur near a European node box, apac for Tokyo."
  type        = string
  default     = "weur"
}

variable "relays" {
  description = "Relay name (DNS label, for example relay-1) to public IPv4 address. Empty in the archive-only tier."
  type        = map(string)
  default     = {}
}

variable "requests_per_10s" {
  description = "Requests (including websocket upgrades) allowed per IP per 10 seconds on live and api."
  type        = number
  default     = 50
}
