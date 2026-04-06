#!/usr/bin/env bash
# Life Agent OS — One-line installer
# Usage: curl -fsSL https://raw.githubusercontent.com/broomva/life/main/install.sh | bash
set -euo pipefail

CYAN='\033[96m'
GREEN='\033[92m'
DIM='\033[2m'
BOLD='\033[1m'
RESET='\033[0m'

echo ""
echo -e "${CYAN}    ██╗     ██╗███████╗███████╗${RESET}"
echo -e "${CYAN}    ██║     ██║██╔════╝██╔════╝${RESET}"
echo -e "${GREEN}    ██║     ██║█████╗  █████╗  ${RESET}"
echo -e "${GREEN}    ██║     ██║██╔══╝  ██╔══╝  ${RESET}"
echo -e "${GREEN}    ███████╗██║██║     ███████╗${RESET}"
echo -e "${CYAN}    ╚══════╝╚═╝╚═╝     ╚══════╝${RESET}"
echo ""
echo -e "    ${DIM}Agent Operating System${RESET}"
echo ""

# Check for Rust/Cargo
if ! command -v cargo &> /dev/null; then
    echo -e "${BOLD}Installing Rust...${RESET}"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
    source "$HOME/.cargo/env"
    echo -e "  ${GREEN}✓${RESET} Rust installed"
fi

echo -e "${BOLD}Installing Life Agent OS...${RESET}"
echo ""

# Try crates.io first, fall back to source
if cargo install life-os 2>/dev/null; then
    echo -e "  ${GREEN}✓${RESET} life-os installed from crates.io"
elif cargo install life-cli 2>/dev/null; then
    echo -e "  ${GREEN}✓${RESET} life-cli installed from crates.io"
else
    echo -e "  ${DIM}crates.io packages not yet available, building from source...${RESET}"
    TMPDIR=$(mktemp -d)
    git clone --depth 1 https://github.com/broomva/life.git "$TMPDIR/life"
    cargo install --path "$TMPDIR/life/crates/cli/life-cli"
    rm -rf "$TMPDIR"
    echo -e "  ${GREEN}✓${RESET} life installed from source"
fi

# Also install arcan (the agent runtime)
echo ""
if cargo install arcan 2>/dev/null; then
    echo -e "  ${GREEN}✓${RESET} arcan installed from crates.io"
else
    echo -e "  ${DIM}arcan not yet on crates.io at latest version, building from source...${RESET}"
    if [ -d "$TMPDIR/life" ]; then
        cargo install --path "$TMPDIR/life/crates/arcan/arcan" 2>/dev/null || true
    fi
fi

echo ""
echo -e "${GREEN}${BOLD}Installation complete!${RESET}"
echo ""
echo -e "  ${BOLD}Get started:${RESET}"
echo -e "    ${CYAN}life setup${RESET}     configure your LLM provider"
echo -e "    ${CYAN}arcan chat${RESET}     start the agent TUI"
echo -e "    ${CYAN}arcan shell${RESET}    interactive REPL"
echo ""
echo -e "  ${DIM}https://docs.broomva.tech/docs/life${RESET}"
echo ""
