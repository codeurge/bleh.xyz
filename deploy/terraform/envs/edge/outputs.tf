output "s3_endpoint" {
  description = "S3_ENDPOINT for deploy/compose/.env."
  value       = module.edge.s3_endpoint
}

output "data_url" {
  description = "Public archive base URL."
  value       = module.edge.data_url
}

output "node_tunnel_tokens" {
  description = "Per node box: write to deploy/compose/secrets/tunnel-token.txt on that box."
  value       = { for k, m in module.node_tunnel : k => m.tunnel_token }
  sensitive   = true
}

output "home_tunnel_token" {
  description = "Write to deploy/compose/secrets/tunnel-token.txt on the home-archive server."
  value       = try(module.home_tunnel[0].tunnel_token, null)
  sensitive   = true
}

output "relay_upstream_headers" {
  description = "Contents of deploy/compose/secrets/upstream-headers.txt on every relay."
  value = try(join("\n", [
    "CF-Access-Client-Id: ${cloudflare_zero_trust_access_service_token.relays[0].client_id}",
    "CF-Access-Client-Secret: ${cloudflare_zero_trust_access_service_token.relays[0].client_secret}",
  ]), null)
  sensitive = true
}

output "relay_upstreams" {
  description = "RELAY_UPSTREAMS for the relays' .env."
  value       = join(",", [for k in var.node_boxes : "wss://origin-${k}.${var.domain}/v0/ws"])
}

output "origin_cert_pem" {
  description = "secrets/origin-ca/origin.pem on every relay."
  value       = try(cloudflare_origin_ca_certificate.relays[0].certificate, null)
  sensitive   = true
}

output "origin_key_pem" {
  description = "secrets/origin-ca/origin-key.pem on every relay."
  value       = try(tls_private_key.origin[0].private_key_pem, null)
  sensitive   = true
}
