output "node_box_ips" {
  description = "Public IPv4 of each Tokyo node box (for wg-mesh peers)."
  value = merge(
    var.enable_aws ? { a = module.site_a[0].public_ip } : {},
    var.enable_vultr ? { b = module.site_b[0].public_ip } : {},
  )
}
