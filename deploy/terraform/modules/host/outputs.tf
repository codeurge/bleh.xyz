output "user_data" {
  description = "cloud-init user data for the host."
  value       = local.user_data
}

output "firewall_rules" {
  description = "Inbound rules for the role, for provider firewalls: description, protocol (tcp or udp), from_port, to_port, cidrs."
  value       = concat(local.common_rules, local.role_rules[var.role])
}
