# Virtual parts

How virtual parts are defined and run. Decided 2026-11-16; the reasons are in [PLAN.md](PLAN.md) under Key decisions.

## What a part is

A part model is a data file plus a small script, so a new part needs no change to the program:

- `<model>.json`: the model's description (schema `schema/part-model/v1.schema.json`): its name, the pins it connects to, whether it sits on an I2C bus, the properties a person sets when placing it, the controls they can work while it runs, and the state the app shows.
- `<model>.rhai`: what it does, in [Rhai](https://rhai.rs), a small scripting language embedded in Rust. Scripts are sandboxed: no files, no network, no processes, and limits on operations and memory per call.

The models that ship with the app are built into the program (`core/crates/circuit/models/`). Loading models from elsewhere would need the consent rules that apply to downloaded projects, so it is not offered.

A circuit (`schema/circuit/`) places parts: each refers to a model, connects its pins to GPIOs (or sits on an I2C bus at an address), and sets its properties.

## Where parts run

All of a simulation's parts run in `esp32bench-qemu-host`, the per-session process that loads lcgamboa's QEMU library. It sees every pin change, and it must answer I2C transactions synchronously from QEMU's thread, so the parts live beside it rather than in the core. The core gives the host the circuit, relays part states to the app (`sim.parts`) and passes on controls (`sim.control`). Parts therefore work only where the host runs: ESP32 and ESP32-C3 on lcgamboa's fork.

- Pin events reach scripts on the host's bridge thread, not QEMU's, so a busy pin cannot slow the emulation.
- I2C events reach scripts on QEMU's thread, synchronously: a script's answer is the ACK or the byte read.
- State changes are batched before they leave the host. Display pictures go out at most 20 times a second.

## Script hooks

Every hook is optional. `props` holds the part's properties and `state` its state; both persist between calls.

| Hook | When |
| --- | --- |
| `init()` | Once, when the simulation starts. |
| `pin(name, level, output)` | The firmware changed a connected pin: its level, and whether it is an output. |
| `control(name, value)` | A person worked a control (pressed a button, moved a slider). |
| `i2c_start(read)` | A transaction to this part's address starts. Return `true` to ACK. |
| `i2c_write(byte)` | The firmware wrote a byte. Return `true` to ACK. |
| `i2c_read()` | The firmware reads a byte: return it (0 to 255). |
| `i2c_stop()` | The transaction ended. |

Functions a script can call:

| Function | Does |
| --- | --- |
| `drive(name, level)` | Drive a connected pin as an input, as a button or sensor output would. |
| `show(pixels)` | For display models: the whole picture, one byte per pixel (0 off, 1 on), row by row. |

## Limits, stated plainly

Parts are firmware-level. An LED is lit when its pin drives it; nothing electrical (current, voltage, timing below the emulator's) is modelled. Only what goes over pins and I2C can be modelled: 1-Wire and single-wire timing protocols (DS18B20, DHT22) are not, and SPI parts wait until the fork's SPI hook is tested.
