# MCP Hub launchd Integration (macOS)

This document describes how to run `csa mcp-hub serve` as a user daemon on macOS.

## 1. Create plist

Create `~/Library/LaunchAgents/dev.csa.mcp-hub.plist`:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>Label</key>
    <string>dev.csa.mcp-hub</string>

    <key>ProgramArguments</key>
    <array>
      <string>/usr/local/bin/csa</string>
      <string>mcp-hub</string>
      <string>serve</string>
      <string>--foreground</string>
      <string>--socket</string>
      <string>/tmp/csa-${UID}/mcp-hub.sock</string>
    </array>

    <key>RunAtLoad</key>
    <true/>

    <key>KeepAlive</key>
    <true/>

    <key>StandardOutPath</key>
    <string>/tmp/csa-mcp-hub.out.log</string>

    <key>StandardErrorPath</key>
    <string>/tmp/csa-mcp-hub.err.log</string>
  </dict>
</plist>
```

Adjust the binary path to match your installation.

## 2. Load service

```bash
launchctl load ~/Library/LaunchAgents/dev.csa.mcp-hub.plist
launchctl start dev.csa.mcp-hub
```

## 3. Verify

```bash
csa mcp-hub status --socket /tmp/csa-${UID}/mcp-hub.sock
```

## 4. Stop / unload

```bash
launchctl stop dev.csa.mcp-hub
launchctl unload ~/Library/LaunchAgents/dev.csa.mcp-hub.plist
```
