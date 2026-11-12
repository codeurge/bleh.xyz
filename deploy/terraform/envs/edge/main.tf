# Edge stack (every tier): R2 archive buckets and data.<domain>, relay DNS,
# rate limits and cache rules, the home-archive tunnel (staging API,
# Grafana) and the relays' Origin CA certificate. Optionally, an
# Access-gated tunnel per node box (node_boxes); relays do not need it,
# because they reach origins over the WireGuard overlay.
#
#   Tier 0 archive-only:  relays = {}
#   Tier 1 (ADR 0010):    relays from envs/relays-oci or envs/relays-hcloud,
#                         home_tunnel = true
#
# Credentials: CLOUDFLARE_API_TOKEN in the environment (docs/operations/keys.md).

provider "cloudflare" {}

locals {
  live_hosts = ["live.${var.domain}", "api.${var.domain}"]
}

module "edge" {
  source = "../../modules/cloudflare-edge"

  account_id            = var.account_id
  zone_id               = var.zone_id
  domain                = var.domain
  public_bucket         = var.public_bucket
  private_bucket        = var.private_bucket
  private_storage_class = var.private_storage_class
  r2_location           = var.r2_location
  relays                = var.relays
  requests_per_10s      = var.requests_per_10s
}

# One service token for every relay, admitted by every node-box tunnel.
resource "cloudflare_zero_trust_access_service_token" "relays" {
  count = length(var.node_boxes) > 0 ? 1 : 0

  account_id = var.account_id
  name       = "bleh-relays"
  duration   = "8760h"
}

module "node_tunnel" {
  source   = "../../modules/cloudflare-tunnel"
  for_each = toset(var.node_boxes)

  account_id       = var.account_id
  zone_id          = var.zone_id
  name             = "bleh-node-${each.key}"
  service_token_id = cloudflare_zero_trust_access_service_token.relays[0].id
  ingress = [{
    hostname = "origin-${each.key}.${var.domain}"
    service  = "http://gateway:8080"
    access   = "service_token"
  }]
}

module "home_tunnel" {
  source = "../../modules/cloudflare-tunnel"
  count  = var.home_tunnel ? 1 : 0

  account_id = var.account_id
  zone_id    = var.zone_id
  name       = "bleh-home"
  ingress = [
    {
      hostname = "api-staging.${var.domain}"
      service  = "http://history-api:8081"
      access   = "email"
      emails   = var.operator_emails
    },
    {
      hostname = "grafana.${var.domain}"
      service  = "http://grafana:3000"
      access   = "email"
      emails   = var.operator_emails
    },
  ]
}

# Origin CA certificate for live/api on the relays (Cloudflare SSL mode Full
# (strict)). The private key is stored in Terraform state, which lives in a
# private bucket; see docs/operations/keys.md for the alternative of
# creating it in the dashboard.
resource "tls_private_key" "origin" {
  count = var.create_origin_cert ? 1 : 0

  algorithm   = "ECDSA"
  ecdsa_curve = "P256"
}

resource "tls_cert_request" "origin" {
  count = var.create_origin_cert ? 1 : 0

  private_key_pem = tls_private_key.origin[0].private_key_pem
  dns_names       = local.live_hosts

  subject {
    common_name = local.live_hosts[0]
  }
}

resource "cloudflare_origin_ca_certificate" "relays" {
  count = var.create_origin_cert ? 1 : 0

  csr                = tls_cert_request.origin[0].cert_request_pem
  hostnames          = local.live_hosts
  request_type       = "origin-ecc"
  requested_validity = 5475
}
