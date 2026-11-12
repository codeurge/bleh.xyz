output "relay_ips" {
  description = "Relay name to public IPv4 address (input to the edge stack)."
  value       = { for k, v in hcloud_server.relay : k => v.ipv4_address }
}
