#!/bin/bash
# Publish Titan to zapstore.dev
#
# Prerequisites:
#   - nak (nostr army knife) installed
#   - NSEC environment variable set
#   - Release artifacts uploaded to GitHub
#
# Usage:
#   NSEC=nsec1... ./scripts/publish-zapstore.sh v0.1.0

set -e

VERSION="${1:?Usage: $0 <version>}"
REPO="btcjt/titan"
TAG="$VERSION"

echo "Publishing Titan $VERSION to zapstore..."

# Zapstore app listing (kind 32267)
# See: https://github.com/nicely/zapstore
nak event \
  -k 32267 \
  --sec "$NSEC" \
  -c "Titan — A native nsite:// browser for the Nostr web. Resolves Bitcoin-registered names to websites hosted on Nostr relays and Blossom servers. No DNS, no certificates, no hosting providers." \
  -d "com.titan.browser" \
  -t name="Titan" \
  -t version="$VERSION" \
  -t "url=https://github.com/$REPO/releases/tag/$TAG" \
  -t "repository=https://github.com/$REPO" \
  -t "license=MIT" \
  -t "t=browser" \
  -t "t=nostr" \
  -t "t=nsite" \
  -t "t=bitcoin" \
  wss://relay.westernbtc.com wss://relay.primal.net wss://relay.damus.io

echo ""
echo "Published to zapstore!"
echo "View at: https://zapstore.dev"
