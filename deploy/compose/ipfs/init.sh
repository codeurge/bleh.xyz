#!/bin/sh
# Kubo configuration for the archive pinning node, run by the image before
# the daemon starts (/container-init.d).
#
# - Swarm on 4101 (TCP and QUIC), never 4001, which Hyperliquid gossip uses.
# - Filestore on: the archive process adds mirror files with --nocopy, so the
#   blocks reference /srv/archive instead of duplicating terabytes.
# - API inside the compose network only (published on 127.0.0.1); public
#   gateway disabled: HTTPS access is data.bleh.xyz.
# - Without an inbound port forward the node relies on hole punching and
#   relays (AutoNAT); forwarding 4101 improves reachability.
set -eu
ipfs config --json Addresses.Swarm '["/ip4/0.0.0.0/tcp/4101","/ip4/0.0.0.0/udp/4101/quic-v1","/ip6/::/tcp/4101","/ip6/::/udp/4101/quic-v1"]'
ipfs config Addresses.API /ip4/0.0.0.0/tcp/5001
ipfs config Addresses.Gateway /ip4/127.0.0.1/tcp/8080
ipfs config --json Experimental.FilestoreEnabled true
ipfs config --json Swarm.RelayClient.Enabled true
ipfs config --json Swarm.ConnMgr.HighWater 400
ipfs config --json Swarm.ConnMgr.LowWater 200
ipfs config Provide.Strategy pinned
