output "relay_ips" {
  description = "Relay name to public IPv4 address (input to the edge stack)."
  value       = { for k, v in oci_core_instance.relay : k => v.public_ip }
}
