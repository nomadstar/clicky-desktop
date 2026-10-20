#!/usr/bin/env bash
# Helper script for Waybar custom/clicky module
# Outputs JSON formatted for Waybar status bar

if command -v clicky-desktop >/dev/null 2>&1; then
    clicky-desktop --status-json
elif [ -f "./target/release/clicky-desktop" ]; then
    ./target/release/clicky-desktop --status-json
elif [ -f "./target/debug/clicky-desktop" ]; then
    ./target/debug/clicky-desktop --status-json
else
    # Fallback status if clicky-desktop binary is not found
    echo '{"text":"󰚩","alt":"stopped","tooltip":"Clicky (Detenido)","class":"stopped"}'
fi
