terraform {
  required_version = ">= 1.10"

  # terraform init -backend-config=../../backend.hcl -backend-config="key=bleh/relays-oci.tfstate"
  backend "s3" {}

  required_providers {
    oci = {
      source  = "oracle/oci"
      version = "~> 7.0"
    }
  }
}
