# Cloudflare edge for bleh.xyz: the archive buckets on R2 (zero egress) with
# data.<domain> as the CDN-fronted custom domain, DNS for the relays,
# per-IP rate limiting and cache rules for the immutable archive.
#
# live.<domain> and api.<domain> are proxied A records, one per relay
# (Cloudflare spreads clients across them). Each relay also has a DNS-only
# name, relay-N.<domain>, that clients can use directly if the proxy is
# unavailable. A health-checked Cloudflare Load Balancer is the Tier 2
# upgrade (docs/operations/hosting.md).

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
  data_host = "data.${var.domain}"
  live_host = "live.${var.domain}"
  api_host  = "api.${var.domain}"

  # Objects the publisher rewrites in place (archive::publish put_mutable,
  # the Iceberg version hint) and web snapshots: cached for a minute. All
  # other archive objects are write-once.
  rewritten_paths = join(" or ", concat(
    [for f in ["/latest.json", "/version-hint.text", "/_manifests/index.json", "/_manifests/magnets.txt", "/manifest.anchors.json", "/manifest.json.ots"] :
    "ends_with(http.request.uri.path, \"${f}\")"],
    ["starts_with(http.request.uri.path, \"/web/\")"],
  ))
}

# ------------------------------------------------------------------ R2

resource "cloudflare_r2_bucket" "public" {
  account_id    = var.account_id
  name          = var.public_bucket
  location      = var.r2_location
  storage_class = "Standard"
}

# Unfiltered raw (contains owner addresses): never public, no custom domain.
resource "cloudflare_r2_bucket" "private" {
  account_id    = var.account_id
  name          = var.private_bucket
  location      = var.r2_location
  storage_class = var.private_storage_class
}

resource "cloudflare_r2_custom_domain" "data" {
  account_id  = var.account_id
  bucket_name = cloudflare_r2_bucket.public.name
  domain      = local.data_host
  zone_id     = var.zone_id
  enabled     = true
  min_tls     = "1.2"
}

# Browsers (DuckDB-WASM) read Parquet with range requests from any origin.
resource "cloudflare_r2_bucket_cors" "public" {
  account_id  = var.account_id
  bucket_name = cloudflare_r2_bucket.public.name
  rules = [{
    allowed = {
      methods = ["GET", "HEAD"]
      origins = ["*"]
      headers = ["Range", "If-None-Match", "If-Modified-Since"]
    }
    expose_headers  = ["Content-Length", "Content-Range", "ETag", "Last-Modified"]
    max_age_seconds = 86400
  }]
}

# ------------------------------------------------------------------ DNS

resource "cloudflare_dns_record" "relay" {
  for_each = var.relays

  zone_id = var.zone_id
  name    = "${each.key}.${var.domain}"
  type    = "A"
  content = each.value
  ttl     = 300
  proxied = false
  comment = "bleh relay (direct)"
}

resource "cloudflare_dns_record" "live" {
  for_each = var.relays

  zone_id = var.zone_id
  name    = local.live_host
  type    = "A"
  content = each.value
  ttl     = 1
  proxied = true
  comment = "bleh live gateway via relay ${each.key}"
}

resource "cloudflare_dns_record" "api" {
  for_each = var.relays

  zone_id = var.zone_id
  name    = local.api_host
  type    = "A"
  content = each.value
  ttl     = 1
  proxied = true
  comment = "bleh history API via relay ${each.key}"
}

# ------------------------------------------------------------------ rules

# Free plan: one rate-limiting rule, 10 s period, 10 s mitigation, counted
# per IP per data centre. Websocket connection caps per IP are enforced by
# the gateway and relays themselves (docs/protocol/gateway-v0.md, Limits).
resource "cloudflare_ruleset" "rate_limit" {
  zone_id     = var.zone_id
  name        = "bleh rate limits"
  description = "Per-IP request limits for the live and API hostnames"
  kind        = "zone"
  phase       = "http_ratelimit"

  rules = [{
    ref         = "bleh_live_api_per_ip"
    description = "Requests and websocket upgrades per IP"
    expression  = "(http.host in {\"${local.live_host}\" \"${local.api_host}\"})"
    action      = "block"
    enabled     = true
    ratelimit = {
      characteristics     = ["cf.colo.id", "ip.src"]
      period              = 10
      requests_per_period = var.requests_per_10s
      mitigation_timeout  = 10
    }
  }]
}

# Archive files are write-once, so they are cached for a year; files the
# publisher rewrites (manifest index, latest pointer, magnets, anchors, OTS
# proofs, Iceberg version hint) and web snapshots (web/, ADR 0016) for a
# minute. A rewritten file cached for a year breaks chain verification.
resource "cloudflare_ruleset" "cache" {
  zone_id     = var.zone_id
  name        = "bleh archive cache"
  description = "Edge caching for data.${var.domain}"
  kind        = "zone"
  phase       = "http_request_cache_settings"

  rules = [
    {
      ref         = "bleh_archive_pointers"
      description = "Short TTL for files that are rewritten and web snapshots"
      expression  = "(http.host eq \"${local.data_host}\" and (${local.rewritten_paths}))"
      action      = "set_cache_settings"
      enabled     = true
      action_parameters = {
        cache = true
        edge_ttl = {
          mode    = "override_origin"
          default = 60
        }
        browser_ttl = {
          mode    = "override_origin"
          default = 30
        }
      }
    },
    {
      ref         = "bleh_archive_immutable"
      description = "Long TTL for immutable archive files"
      expression  = "(http.host eq \"${local.data_host}\" and not (${local.rewritten_paths}))"
      action      = "set_cache_settings"
      enabled     = true
      action_parameters = {
        cache = true
        edge_ttl = {
          mode    = "override_origin"
          default = 31536000
          # Only a 200 is immutable. A 404 for a day not yet published (a
          # client polling for tomorrow's manifest) must not stick at the
          # edge for a year and hide the file once it is uploaded.
          status_code_ttl = [
            {
              status_code_range = { from = 300, to = 599 }
              value             = 30
            },
          ]
        }
        browser_ttl = {
          mode    = "override_origin"
          default = 86400
        }
      }
    },
  ]
}
