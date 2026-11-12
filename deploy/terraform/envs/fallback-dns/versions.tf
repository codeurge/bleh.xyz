terraform {
  required_version = ">= 1.10"

  # terraform init -backend-config=../../backend.hcl -backend-config="key=bleh/fallback-dns.tfstate"
  backend "s3" {}

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
  }
}
