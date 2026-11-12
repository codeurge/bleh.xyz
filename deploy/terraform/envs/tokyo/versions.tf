terraform {
  required_version = ">= 1.10"

  # terraform init -backend-config=../../backend.hcl -backend-config="key=bleh/tokyo.tfstate"
  backend "s3" {}

  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
    vultr = {
      source  = "vultr/vultr"
      version = "~> 2.27"
    }
  }
}
