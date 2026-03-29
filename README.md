# esp32-universal-control

Share your Mac's keyboard and trackpad with any device over Bluetooth. An ESP32-S3 bridges Mac input to BLE HID, so targets see a native keyboard and precision touchpad. No drivers needed on the receiving side.

```
Mac --USB--> ESP32-S3 --BLE--> Windows / Linux / Android
```

## Setup

Requires the [ESP32 Rust toolchain](https://docs.espressif.com/projects/rust/book/getting-started/toolchain.html).

```sh
cd firmware && cargo run        # flash + monitor
cd host && cargo run -- run <serial-port>   # capture mode
cd host && cargo run -- debug <serial-port> # debug CLI
```

The host binary needs two macOS permissions (System Settings > Privacy & Security):
- **Input Monitoring**: to capture keyboard events
- **Accessibility**: to suppress keyboard and mouse events when forwarding

## Switching targets

| Shortcut | Target |
|---|---|
| Ctrl+Opt+1 | Mac (default) |
| Ctrl+Opt+2 | Remote slot 0 |
| Ctrl+Opt+3 | Remote slot 1 |
| Ctrl+Opt+4 | Remote slot 2 |
| Ctrl+Opt+5 | Remote slot 3 |

When forwarding to a remote device, keyboard and mouse input is suppressed on Mac. If the ESP32 disconnects, input reverts to Mac automatically.

## Project structure

| Crate | Purpose |
|---|---|
| `firmware/` | ESP32-S3 BLE HID device (NimBLE), UART receive, report forwarding |
| `host/` | macOS input capture (CGEventTap, MultitouchSupport.framework), serial output |
| `protocol/` | Shared `no_std` types: HID report structs, wire protocol messages |

## Known issues

- Three-finger system gestures (Mission Control, App Expose) cannot be blocked from userspace. Disable in System Settings > Trackpad if needed.
- Left/right modifier keys are not distinguished (macOS CGEventFlags limitation).

## License

See [LICENSE](LICENSE).
