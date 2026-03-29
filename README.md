# Paintress

Paintress is a swarm orchestrator of sorts to manage e-ink
display casting to esp32 arduinos.

The features allow for OTA updates once the Arduinos are flashed
the first time.

It also auto-discovers systems that connect to the same wifi and 
allows one to control positioning, direction and define a grid.

# Requirements

- Cargo
- Some way to flash esp32 with the webserver.ino

# Usage

## Workflow
 paintress discover              # scan network, create/update paintress.toml
  paintress send photo.jpg        # dither + send to your display wall
  paintress send photo.jpg --preview  # save preview.png instead of sending
  paintress preview photo.jpg     # offline preview (no network needed)

## Fleet management
  paintress status                # query display health/uptime
  paintress ota firmware.bin      # OTA update all displays
  paintress ota firmware.bin --to f8c0a8  # OTA a specific display

## Global options (before the subcommand)
  paintress --timeout 5 discover          # longer mDNS scan
  paintress --saturation 2.0 send pic.jpg # boost colors
  paintress --saturation 1.0 send pic.jpg # no color boost
