output "public_bucket" {
  description = "Public archive bucket name."
  value       = cloudflare_r2_bucket.public.name
}

output "private_bucket" {
  description = "Private raw bucket name."
  value       = cloudflare_r2_bucket.private.name
}

output "s3_endpoint" {
  description = "S3-compatible endpoint for R2 (S3_ENDPOINT in deploy/compose/.env)."
  value       = "https://${var.account_id}.r2.cloudflarestorage.com"
}

output "data_url" {
  description = "Public HTTPS base of the archive."
  value       = "https://${cloudflare_r2_custom_domain.data.domain}"
}
