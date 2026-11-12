# A remotely managed Cloudflare Tunnel with its ingress rules, DNS records
# and an Access application per hostname. Used for the node box (origin
# gateway, reachable by relays only) and the home-archive server (staging
# API, Grafana): outbound-only publishing, no inbound ports, origin
# addresses not published.
#
# Every hostname gets an Access policy: "service_token" admits machine
# clients holding the module's service token (relays send it as
# CF-Access-Client-Id / CF-Access-Client-Secret headers); "email" admits
# the listed people after a one-time PIN or identity-provider login.

terraform {
  required_version = ">= 1.10"
  required_providers {
    cloudflare = {
      source  = "cloudflare/cloudflare"
      version = "~> 5.12"
    }
  }
}

locals {
  ingress_by_host = { for i in var.ingress : i.hostname => i }
  needs_token     = anytrue([for i in var.ingress : i.access == "service_token"])
  own_token       = local.needs_token && var.service_token_id == null
  token_id        = local.own_token ? cloudflare_zero_trust_access_service_token.this[0].id : var.service_token_id
  emails          = distinct(flatten([for i in var.ingress : i.emails]))
}

resource "cloudflare_zero_trust_tunnel_cloudflared" "this" {
  account_id = var.account_id
  name       = var.name
  config_src = "cloudflare"
}

data "cloudflare_zero_trust_tunnel_cloudflared_token" "this" {
  account_id = var.account_id
  tunnel_id  = cloudflare_zero_trust_tunnel_cloudflared.this.id
}

resource "cloudflare_zero_trust_tunnel_cloudflared_config" "this" {
  account_id = var.account_id
  tunnel_id  = cloudflare_zero_trust_tunnel_cloudflared.this.id

  config = {
    ingress = concat(
      [for i in var.ingress : {
        hostname = i.hostname
        service  = i.service
        origin_request = {
          connect_timeout    = 10
          keep_alive_timeout = 120
        }
      }],
      [{ service = "http_status:404" }],
    )
  }
}

resource "cloudflare_dns_record" "this" {
  for_each = local.ingress_by_host

  zone_id = var.zone_id
  name    = each.key
  type    = "CNAME"
  content = "${cloudflare_zero_trust_tunnel_cloudflared.this.id}.cfargotunnel.com"
  ttl     = 1
  proxied = true
  comment = "bleh tunnel ${var.name}"
}

# ------------------------------------------------------------------ Access

resource "cloudflare_zero_trust_access_service_token" "this" {
  count = local.own_token ? 1 : 0

  account_id = var.account_id
  name       = "${var.name}-clients"
  duration   = var.service_token_duration
}

resource "cloudflare_zero_trust_access_policy" "service_token" {
  count = local.needs_token ? 1 : 0

  account_id = var.account_id
  name       = "${var.name} service token"
  decision   = "non_identity"
  include = [{
    service_token = {
      token_id = local.token_id
    }
  }]
}

resource "cloudflare_zero_trust_access_policy" "email" {
  count = length(local.emails) > 0 ? 1 : 0

  account_id = var.account_id
  name       = "${var.name} people"
  decision   = "allow"
  include    = [for e in local.emails : { email = { email = e } }]
}

resource "cloudflare_zero_trust_access_application" "this" {
  for_each = local.ingress_by_host

  account_id       = var.account_id
  name             = each.key
  domain           = each.key
  type             = "self_hosted"
  session_duration = "24h"

  policies = [{
    id = (
      each.value.access == "service_token"
      ? cloudflare_zero_trust_access_policy.service_token[0].id
      : cloudflare_zero_trust_access_policy.email[0].id
    )
    precedence = 1
  }]
}
