output "live_url" {
  description = "Fallback websocket URL to list in the registry and SDKs."
  value       = "wss://live.${var.domain}/v0/ws"
}
