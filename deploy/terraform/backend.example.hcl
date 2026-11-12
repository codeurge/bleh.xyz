# Remote state on a private Cloudflare R2 bucket (S3 backend). Copy to
# backend.hcl (git-ignored) and initialise each stack with its own key:
#
#   cd deploy/terraform/envs/edge
#   export AWS_ACCESS_KEY_ID=... AWS_SECRET_ACCESS_KEY=...   # R2 token for the state bucket only
#   terraform init -backend-config=../../backend.hcl -backend-config="key=bleh/edge.tfstate"
#
# Create the state bucket by hand first (it cannot manage itself), keep it
# private and give its token to nobody but operators. State contains tunnel
# tokens and the Origin CA key: treat it as a secret. OpenTofu works the same way and can also encrypt state at rest
# (docs/operations/deploy.md).

bucket = "bleh-tfstate"
region = "auto"

endpoints = {
  s3 = "https://ACCOUNT_ID.r2.cloudflarestorage.com"
}

skip_credentials_validation = true
skip_region_validation      = true
skip_requesting_account_id  = true
skip_metadata_api_check     = true
skip_s3_checksum            = true
use_path_style              = true
# Conditional-write locking (Terraform 1.10+); R2 supports If-None-Match.
use_lockfile = true
