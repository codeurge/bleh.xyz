# Copy to production.tfvars.hcl (git-ignored) and apply with
#   terraform apply -var-file=production.tfvars.hcl
# Values here are placeholders.

account_id = "0123456789abcdef0123456789abcdef"
zone_id    = "fedcba9876543210fedcba9876543210"
domain     = "bleh.xyz"

# Near the node box (ADR 0010: Europe).
r2_location = "weur"

# From the relays stack: terraform -chdir=../relays-oci output -json relay_ips
relays = {
  relay-1 = "192.0.2.11"
  relay-2 = "192.0.2.12"
}

# Tier 1: one node box. Tier 2+: ["a", "b"].
node_boxes = ["a"]

home_tunnel     = true
operator_emails = ["ops@example.org"]
