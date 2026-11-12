output "tunnel_id" {
  description = "Tunnel ID."
  value       = cloudflare_zero_trust_tunnel_cloudflared.this.id
}

output "tunnel_token" {
  description = "Token for cloudflared (deploy/compose/secrets/tunnel-token.txt on the host)."
  value       = data.cloudflare_zero_trust_tunnel_cloudflared_token.this.token
  sensitive   = true
}

output "service_token_client_id" {
  description = "Access service token client ID (relays: CF-Access-Client-Id)."
  value       = try(cloudflare_zero_trust_access_service_token.this[0].client_id, null)
  sensitive   = true
}

output "service_token_client_secret" {
  description = "Access service token client secret (relays: CF-Access-Client-Secret)."
  value       = try(cloudflare_zero_trust_access_service_token.this[0].client_secret, null)
  sensitive   = true
}
