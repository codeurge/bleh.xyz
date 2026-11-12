output "public_ip" {
  description = "Public IPv4 of the bare-metal node box."
  value       = vultr_bare_metal_server.this.main_ip
}
