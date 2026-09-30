# Attack Shark X11 configuration protocol

This is how the X11 is configured over USB HID. It is based on community reverse-engineering (see
[Sources](#sources)). Everything below was verified against a real X11 over the 2.4 GHz dongle, unless it is marked
otherwise. The implementation is `crates/core/src/protocol.rs` (wire format) and `crates/core/src/device.rs`
(transport and retries).

Byte offsets count from 0 and include the leading report-ID byte. Multi-byte checksums are big-endian.

## Devices

| | Vendor | Product | Name |
|---|---|---|---|
| 2.4 GHz dongle | `1d57` | `fa60` | 2.4G Wireless Device |
| USB-C cable | `1d57` | `fa55` | USB Gaming Mouse |

The protocol is the same over both; the cable path is untested here. Bluetooth uses a different, GATT-based channel,
which this project doesn't implement.

## Transport

The configuration protocol lives on **USB interface 2**. On Windows that interface appears as four top-level
collections. Two of them matter:

| Usage page | Direction | Used for |
|---|---|---|
| `0x0B` | Feature reports | Writing settings, and the read handshake |
| `0x0A` | Input report `0x03`, 5 bytes | Events: battery, write acknowledgements, DPI-button and profile changes |

The `0x0B` collection declares feature reports `0x04`–`0x0C`, `0x10`, `0x22` and `0xA0`. Windows sizes every one
of them to **64 bytes**, and pads shorter payloads with zeros. On Linux both collections share one hidraw node.

**Never send report `0x0B`.** It has been observed to unpair the dongle. To re-pair: switch the mouse off and on,
then hold the DPI button until the dongle's LED stops blinking.

## Reading a report

A report can't be read directly. Reading it without permission returns garbage.

1. **Request.** Send feature report `0xA0` = `a0 <id> <length> 00 <param> 00 00 00`.
   - `<length>` must be the report's exact length.
   - `<param>` is the profile (1–5) for profile-scoped reports, and `0` for the profile report.
2. **Poll.** Read feature report `0xA0` until byte 1 is `01`. That means unlocked, typically after 30–220 ms.
3. **Read.** Read feature report `<id>`.

One request unlocks exactly one read.

## Writing a report

Send the report as a feature report. The mouse verifies the checksum and answers with an event:

- `03 55 50 00 <id>` means accepted;
- `03 55 50 01 <id>` means rejected, for example on a bad checksum.

Settings are stored on the mouse, so they survive power cycles and switching between dongle and cable.

## Reliability over the 2.4 GHz link

These were measured with the mouse at rest on a desk:

- **Dropped requests.** About 1 in 30 read requests is never answered. Re-send the request after about 0.75 s.
- **Silences.** Now and then the mouse answers nothing for several seconds; 6 s has been measured. Battery events
  keep arriving during these silences. A read therefore needs a deadline of about 10 s, not a fixed number of tries.
- **Lost ACKs.** A write can be applied without its ACK ever arriving. Settle it by reading the report back.
- **Stale events.** Drain queued events before writing, so an old ACK can't be mistaken for the new one.

**Windows:** a thread's pending overlapped HID reads are cancelled when the thread exits. Keep all device I/O on one
long-lived thread.

## Reports

### `0x0C` profiles (10 bytes)

| Byte | Meaning |
|---|---|
| 2 | Active profile, 1-based |
| 3 | `~byte2` |
| 4 | Number of profiles (5 on this mouse) |
| 5 | `~byte4` |

Unlike the other reports, byte 2 here is the active profile, not the profile the report belongs to. Read it with
param `0`.

### `0x06` polling rate (9 bytes)

| Byte | Meaning |
|---|---|
| 2 | Profile |
| 3 | Rate: `08` = 125 Hz, `04` = 250, `02` = 500, `01` = 1000 |
| 4 | `0xFF - byte3` |

### `0x04` DPI stages and sensor (56 bytes)

| Byte | Meaning |
|---|---|
| 2 | Profile |
| 3 | Low nibble: angle snap (1 = on). High nibble: lift-off distance (unsupported by the X11). |
| 4 | Low nibble: ripple control (1 = on). High nibble: motion sync (unsupported). |
| 5 | Bitmask of the stages the DPI button cycles through (`0x3F` = all 6) |
| 6, 7 | Two copies of the per-stage "double" mask (bit *n* = stage *n*+1) |
| 8–15 | Per-stage DPI code (stages 1–8; the X11 uses 6) |
| 16–23 | Per-stage "high" flag |
| 24 | Active stage, 1-based |
| 25–48 | Per-stage LED color, RGB (8 × 3 bytes) |
| 49 | DPI indicator style (kept as read) |
| 50–51 | `sum(bytes 3..=49) & 0xFFFF` |

**DPI encoding.**

- The base code table covers 50–10000 DPI in steps of 50 (PAW3311 register values, roughly `dpi × 3 / 128`).
- Higher values reuse it with ×2 multipliers:
  - 10100–12000: the base code plus the "high" flag;
  - 12100–20000: the base code plus the "double" mask bit;
  - 20200–22000: both.
- That is why the supported steps are 50 up to 10000, 100 up to 20000, and 200 above.
- The factory stage 6 is `0x81` (5500) × 2 × 2 = 22000.

### `0x05` lighting, sleep and debounce (15 bytes)

| Byte | Meaning |
|---|---|
| 2 | Profile |
| 3 | High nibble: mode. `0` off, `1` static, `2` breathing, `3` neon, `4` color breathing, `5` static in the stage color, `6` breathing in the stage color |
| 4 | High nibble: deep-sleep minutes, high nibble. Low nibble: `6 - speed` (speed 1–5). |
| 5 | High nibble: deep-sleep minutes, low nibble. Low nibble: brightness 1–8. |
| 6–8 | Color, RGB |
| 9 | Sleep time in half-minutes (1–60) |
| 10 | Debounce in ms ÷ 2 (4–50 ms) |
| 11–12 | `sum(bytes 3..=10) & 0xFFFF` |

The speed direction (`6 - speed`) follows OpenSharkX11's observation, and hasn't been checked by eye here.

### `0x08` button bindings (59 bytes)

Eighteen 3-byte slots start at byte 3: slot *n* is at bytes `3n..3n+2`. The checksum is at bytes 57–58:
`sum(bytes 3..=56) & 0xFFFF`. A read returns the slots in the same order they are written.

Each slot holds `<action> <modifiers> <key>`. The X11's buttons use these slots:

| Slot | Button | Default action |
|---|---|---|
| 1 | Left | `02` left click |
| 2 | Right | `03` right click |
| 3 | Wheel click | `04` middle click |
| 6 | DPI (underside) | `0d` DPI cycle |
| 7 | Side, front | `06` forward |
| 8 | Side, rear | `05` back |
| 17 | Wheel up | `09` scroll up |
| 18 | Wheel down | `0a` scroll down |

The unused slots read as `00`.

Actions:

| Code | Actions |
|---|---|
| `01` | disabled |
| `02`–`07` | left, right and middle click, back, forward, double click |
| `09`, `0a` | scroll up, scroll down |
| `0d`–`0f` | DPI cycle, up, down |
| `11` | key: modifiers in byte 2 (`01` ctrl, `02` shift, `04` alt, `08` win), USB HID usage in byte 3 |
| `15`–`1e` | media player, previous, next, play/pause, stop, mute, volume up and down, calculator, email |
| `20`–`26` | browser forward, back, stop, this PC, refresh, home, search |
| `34`–`36` | profile cycle, up, down |
| `40` | polling-rate cycle |

Fire (`08`), easy aim (`10`) and macros (`12`) need parameters or macro data, which aren't covered here.

## Events (input report `0x03`)

Every event is `03 55 <code> <p1> <p2>`. Byte 1 is the model ID; `55` is the X11.

| Code | Meaning |
|---|---|
| `40` / `41` | Battery. `p1`: `01` discharging, `02` fully charged, `03` charging. `p2`: percent. Sent about every 2 s while the mouse is on. |
| `50` | Write acknowledgement. `p1`: `00` accepted, `01` rejected. `p2`: report ID. |
| `10` | DPI button pressed. `p1`: the new stage. |
| `80` | Active profile changed on the mouse. `p1`: the profile, 0-based. |

Other codes (`00`, `ff`, …) show up occasionally, and their meaning is unknown.

## Sources

- [HarukaYamamoto0/attack-shark-x11-driver](https://github.com/HarukaYamamoto0/attack-shark-x11-driver): the read
  handshake, events, and buttons and profile reports.
- [clevim/OpenSharkX11](https://github.com/clevim/OpenSharkX11): DPI tables, lighting, the Bluetooth channel, and the
  report `0x0B` warning.
- [libratbag#1807](https://github.com/libratbag/libratbag/issues/1807): early USB captures of the official software.
