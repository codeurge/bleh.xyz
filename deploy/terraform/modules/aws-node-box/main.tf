# Tier 3 node box on AWS in Tokyo (ap-northeast-1), close to many
# Hyperliquid peers (verify with deploy/scripts/peer-latency.sh). One
# instance with local NVMe instance storage for node output (lost on
# stop/start, which is acceptable: the node resyncs from peers and output is
# captured hourly), an Elastic IP, and a security group built from the host
# module's rules. Public traffic goes through relays, so AWS egress stays
# small: the reader stream to the other origin and raw uploads to R2.

terraform {
  required_version = ">= 1.10"
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "~> 6.0"
    }
  }
}

module "host" {
  source = "../host"

  role                = "node-box"
  site                = var.site
  overlay_ip          = var.overlay_ip
  repo_url            = var.repo_url
  repo_ref            = var.repo_ref
  ssh_authorized_keys = var.ssh_authorized_keys
  ssh_cidrs           = var.ssh_cidrs
}

data "aws_availability_zones" "available" {
  state = "available"
}

data "aws_ami" "ubuntu" {
  most_recent = true
  owners      = ["099720109477"] # Canonical

  filter {
    name   = "name"
    values = ["ubuntu/images/hvm-ssd-gp3/ubuntu-noble-24.04-amd64-server-*"]
  }
}

resource "aws_vpc" "this" {
  cidr_block           = "10.61.0.0/16"
  enable_dns_hostnames = true
  tags                 = { Name = "bleh-${var.site}" }
}

resource "aws_internet_gateway" "this" {
  vpc_id = aws_vpc.this.id
  tags   = { Name = "bleh-${var.site}" }
}

resource "aws_subnet" "this" {
  vpc_id                  = aws_vpc.this.id
  cidr_block              = "10.61.1.0/24"
  availability_zone       = data.aws_availability_zones.available.names[0]
  map_public_ip_on_launch = false
  tags                    = { Name = "bleh-${var.site}" }
}

resource "aws_route_table" "this" {
  vpc_id = aws_vpc.this.id

  route {
    cidr_block = "0.0.0.0/0"
    gateway_id = aws_internet_gateway.this.id
  }

  tags = { Name = "bleh-${var.site}" }
}

resource "aws_route_table_association" "this" {
  subnet_id      = aws_subnet.this.id
  route_table_id = aws_route_table.this.id
}

resource "aws_security_group" "this" {
  name        = "bleh-${var.site}-node-box"
  description = "bleh node box (${var.site})"
  vpc_id      = aws_vpc.this.id
}

resource "aws_vpc_security_group_ingress_rule" "this" {
  for_each = {
    for x in flatten([
      for r in module.host.firewall_rules : [
        for c in r.cidrs : merge(r, { cidr = c })
      ]
    ]) : "${x.protocol}-${x.from_port}-${x.to_port}-${x.cidr}" => x
  }

  security_group_id = aws_security_group.this.id
  description       = each.value.description
  ip_protocol       = each.value.protocol
  from_port         = each.value.from_port
  to_port           = each.value.to_port
  cidr_ipv4         = each.value.cidr
}

resource "aws_vpc_security_group_egress_rule" "all" {
  security_group_id = aws_security_group.this.id
  description       = "All outbound"
  ip_protocol       = "-1"
  cidr_ipv4         = "0.0.0.0/0"
}

resource "aws_key_pair" "this" {
  key_name   = "bleh-${var.site}"
  public_key = var.ssh_authorized_keys[0]
}

resource "aws_instance" "this" {
  ami                    = data.aws_ami.ubuntu.id
  instance_type          = var.instance_type
  subnet_id              = aws_subnet.this.id
  vpc_security_group_ids = [aws_security_group.this.id]
  key_name               = aws_key_pair.this.key_name
  user_data              = module.host.user_data
  ebs_optimized          = true

  metadata_options {
    http_tokens   = "required"
    http_endpoint = "enabled"
  }

  root_block_device {
    volume_type = "gp3"
    volume_size = var.root_volume_gb
    encrypted   = true
  }

  tags = { Name = "bleh-${var.site}-node-box" }

  lifecycle {
    ignore_changes = [ami, user_data]
  }
}

resource "aws_eip" "this" {
  domain   = "vpc"
  instance = aws_instance.this.id
  tags     = { Name = "bleh-${var.site}" }
}
