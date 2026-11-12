output "relay_ips" {
  description = "Feed into envs/edge as relays."
  value       = module.relays.relay_ips
}
