#!/bin/bash
# Build script for Ferrous Kernel
#
# This script builds the Ferrous kernel and its dependencies.

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}Building Ferrous Kernel...${NC}"

# Build the kernel
cargo build "$@"

echo -e "${GREEN}Build complete!${NC}"

