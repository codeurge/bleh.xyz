terraform {
  required_version = ">= 1.10"

  # terraform init -backend-config=../../backend.hcl -backend-config="key=bleh/relays-hcloud.tfstate"
  backend "s3" {}

  required_providers {
    hcloud = {
      source  = "hetznercloud/hcloud"
      version = "~> 1.52"
    }
  }
}
