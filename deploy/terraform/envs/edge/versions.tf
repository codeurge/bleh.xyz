terraform {
  required_version = ">= 1.10"

  # Partial configuration: terraform init -backend-config=../../backend.hcl
  # -backend-config="key=bleh/edge.tfstate" (see ../../backend.example.hcl).
  backend "s3" {}

  required_providers {
    cloudflare = {
      source  = "cloudflare/cloudflare"
      version = "~> 5.12"
    }
    tls = {
      source  = "hashicorp/tls"
      version = "~> 4.1"
    }
  }
}
