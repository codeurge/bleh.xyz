# Takedown resistance: a second domain on a different registrar and DNS
# provider (Route 53 here), pointing straight at the relays with no CDN in
# between. If the primary domain or its DNS is taken down, SDKs and the
# on-chain registry switch to these names (docs/operations/incident.md).
# Costs cents a month plus the domain.

provider "aws" {
  region = "us-east-1"
}

resource "aws_route53_record" "live" {
  zone_id = var.zone_id
  name    = "live.${var.domain}"
  type    = "A"
  ttl     = 60
  records = values(var.relays)
}

resource "aws_route53_record" "api" {
  zone_id = var.zone_id
  name    = "api.${var.domain}"
  type    = "A"
  ttl     = 60
  records = values(var.relays)
}

resource "aws_route53_record" "relay" {
  for_each = var.relays

  zone_id = var.zone_id
  name    = "${each.key}.${var.domain}"
  type    = "A"
  ttl     = 300
  records = [each.value]
}
